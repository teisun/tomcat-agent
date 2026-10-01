import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { describe, expect, it, vi } from "vitest";

import { initializeServe } from "../initialize";
import type { AskQuestionResult } from "../protocol";
import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readLatestCommand(child: FakeChildProcess): Record<string, unknown> {
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
  it("completes initialize handshake via control_response", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
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

  it("auto-answers ask_question via registered handler", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
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
    const firstChild = new FakeChildProcess();
    const secondChild = new FakeChildProcess();
    const children = [firstChild, secondChild];
    const spawnFactory = (() =>
      children.shift() as unknown as ChildProcessWithoutNullStreams) as unknown as typeof import("node:child_process").spawn;
    const messenger = new TomcatMessenger({ executable: "tomcat", spawnFactory });
    const deferredHandlers = [deferred<AskQuestionResult>(), deferred<AskQuestionResult>()];
    const contexts: Array<{ generation: number; signal: AbortSignal }> = [];
    messenger.registerAskQuestionHandler((_request, _frame, context) => {
      contexts.push(context);
      return deferredHandlers[contexts.length - 1].promise;
    });

    messenger.start();
    firstChild.emitStdout(askFrame("ask-old", "s1"));
    await vi.waitFor(() => expect(contexts).toHaveLength(1));
    messenger.restart();
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
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
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
