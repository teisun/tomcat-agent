import type { SharedConnectionOptions } from "../sharedConnection";
import { describe, expect, it, vi } from "vitest";

import { initializeServe } from "../initialize";
import type { AskQuestionResult } from "../protocol";
import { createReadyMessenger } from "./fakes";
import { createConnectionFactory, FakeConnection } from "./fakes";

function readLatestCommand(child: FakeConnection): Record<string, unknown> {
  return JSON.parse(child.readStdin().trim());
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

function askFrame(requestId: string, sessionId: string) {
  return `${JSON.stringify({
    payload: {
      questions: [
        {
          id: "color",
          options: [{ id: "blue", label: "Blue", recommended: true }],
          prompt: "Pick a color",
        },
      ],
      requestId,
      responseEvent: `plan.ask_question.response.${requestId}`,
    },
    requestId,
    sessionId,
    subtype: "ask_question",
    type: "control_request",
  })}\n`;
}

describe("TomcatMessenger control roundtrip", () => {
  it("settles an outbound initialize cancellation immediately without cancelling another ID", async () => {
    vi.useFakeTimers();
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({ executable: "tomcat", connectFactory: createConnectionFactory(child) });
    const rejected = vi.fn();
    const observed = initializeServe(messenger, 1_000).catch(rejected);
    const command = readLatestCommand(child);
    try {
      child.emitStdout(`${JSON.stringify({ type: "control_cancel", requestId: "unrelated" })}\n`);
      await new Promise<void>(resolve => process.nextTick(resolve));
      expect(rejected).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(1);
      child.emitStdout(`${JSON.stringify({ type: "control_cancel", requestId: command.requestId })}\n`);
      await observed;
      expect(rejected).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining("initialize was cancelled") }));
      expect(vi.getTimerCount()).toBe(0);
      await observed;
    } finally {
      messenger.dispose();
      await observed;
      vi.useRealTimers();
    }
  });

  it("completes initialize handshake via control_response", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
    });

    const pending = initializeServe(messenger);
    const command = readLatestCommand(child);

    child.emitStdout(
      `${JSON.stringify({
        payload: {
          capabilities: ["prompt", "ask_question"],
          protocolVersion: 1,
          serverVersion: "0.1.20",
          sessionId: "s-bootstrap",
        },
        requestId: command.requestId,
        sessionId: "s-bootstrap",
        type: "control_response",
      })}\n`,
    );

    await expect(pending).resolves.toEqual({
      attachmentRoot: null,
      slashCommands: [],
      capabilities: ["prompt", "ask_question"],
      protocolVersion: 1,
      serverVersion: "0.1.20",
      sessionId: "s-bootstrap",
    });
  });

  it("initializes a shared connection with explicit cwd and current-or-new semantics", async () => {
    const socket = new FakeConnection();
    const messenger = await createReadyMessenger({ executable: "unused", cwd: "/workspace/project", connectFactory: createConnectionFactory(socket) });
    const initialized = initializeServe(messenger);
    const init = readLatestCommand(socket);
    socket.emitStdout(JSON.stringify({ type: "control_response", requestId: init.requestId, payload: {
      capabilities: ["prompt", "ask_question"], protocolVersion: 2, serverVersion: "0.1.80", sessionId: null,
    } }) + "\n");
    await new Promise<void>(resolve => setImmediate(resolve));
    const bootstrap = readLatestCommand(socket);
    expect(bootstrap).toMatchObject({ type: "new_session", params: { cwd: "/workspace/project", resumeCurrent: true } });
    socket.emitStdout(JSON.stringify({ type: "response", id: bootstrap.id, sessionId: "existing", success: true, payload: { access: "observe" } }) + "\n");
    await expect(initialized).resolves.toMatchObject({ sessionId: "existing" });
    expect(socket.readStdin()).toBe("");
    await messenger.disposeAsync();
  });

  it("auto-answers ask_question via registered handler", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
    });

    messenger.registerAskQuestionHandler(async () => ({
      answers: [
        {
          customText: null,
          optionIds: ["blue"],
          pickedRecommended: true,
          questionId: "color",
          skipped: false,
        },
      ],
      cancelled: false,
    }));

    messenger.start();
    child.emitStdout(askFrame("ask-1", "s1"));

    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(readLatestCommand(child)).toMatchObject({
      requestId: "ask-1",
      sessionId: "s1",
      type: "control_response",
    });
  });

  it("aborts stale deferred handlers across restart and child exit", async () => {
    const firstChild = new FakeConnection();
    const secondChild = new FakeConnection();
    const children = [firstChild, secondChild];
    const connectFactory = (options: SharedConnectionOptions) => createConnectionFactory(children.shift()!)(options);
    const messenger = await createReadyMessenger({ executable: "tomcat", connectFactory });
    const deferredHandlers = [deferred<AskQuestionResult>(), deferred<AskQuestionResult>()];
    const contexts: Array<{ generation: number; signal: AbortSignal }> = [];
    messenger.registerAskQuestionHandler((_request, _frame, context) => {
      contexts.push(context);
      return deferredHandlers[contexts.length - 1].promise;
    });

    messenger.start();
    firstChild.emitStdout(askFrame("ask-old", "s1"));
    await vi.waitFor(() => expect(contexts).toHaveLength(1));
    await messenger.restart();
    expect(contexts[0].signal.aborted).toBe(true);

    deferredHandlers[0].resolve({ answers: [], cancelled: true, outcome: "skipped" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(secondChild.readStdin()).toBe("");

    secondChild.emitStdout(askFrame("ask-new", "s2"));
    await vi.waitFor(() => expect(contexts).toHaveLength(2));
    expect(contexts[1].generation).toBeGreaterThan(contexts[0].generation);
    expect(contexts[1].signal.aborted).toBe(false);
    secondChild.fail(new Error("pipe closed"));
    expect(contexts[1].signal.aborted).toBe(true);

    deferredHandlers[1].resolve({ answers: [], cancelled: true, outcome: "skipped" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(secondChild.readStdin()).toBe("");
    messenger.dispose();
  });

  it("aborts the matching inbound control handler on control_cancel", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
    });
    const wait = deferred<AskQuestionResult>();
    let signal: AbortSignal | undefined;
    messenger.registerAskQuestionHandler((_request, _frame, context) => {
      signal = context.signal;
      return wait.promise;
    });

    messenger.start();
    child.emitStdout(askFrame("ask-cancel", "s1"));
    await vi.waitFor(() => expect(signal).toBeDefined());
    child.emitStdout(
      `${JSON.stringify({
        requestId: "ask-cancel",
        sessionId: "s1",
        type: "control_cancel",
      })}\n`,
    );
    expect(signal?.aborted).toBe(true);

    wait.resolve({ answers: [], cancelled: true, outcome: "skipped" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(child.readStdin()).toBe("");
  });
});
