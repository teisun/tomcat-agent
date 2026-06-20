import { describe, expect, it } from "vitest";

import { ParticipantTurnRenderer } from "../src/ui/participant/render";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import {
  createSpawnFactory,
  FakeChildProcess,
} from "../src/serveClient/tests/fakes";

describe("backpressure notice integration", () => {
  it("surfaces llm_notice while preserving later lifecycle events", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
    });
    const progresses: string[] = [];
    const markdowns: string[] = [];
    const renderer = new ParticipantTurnRenderer(
      {
        async rememberToolResult() {
          throw new Error("not used");
        },
        async rememberToolStart() {
          return undefined;
        },
      } as never,
      {
        markdown(value: string) {
          markdowns.push(value);
        },
        progress(value: string) {
          progresses.push(value);
        },
      } as never,
    );

    let sawAgentEnd = false;
    let renderQueue = Promise.resolve();
    messenger.onEvent((event) => {
      if (event.type === "agent_end") {
        sawAgentEnd = true;
      }
      renderQueue = renderQueue.then(() => renderer.render(event));
    });

    messenger.start();
    child.emitStdout('{"type":"agent_start","sessionId":"s1"}\n');
    for (const delta of ["a", "b", "c", "d"]) {
      child.emitStdout(
        `${JSON.stringify({
          assistantMessageEvent: { delta, kind: "content_delta" },
          message: {},
          sessionId: "s1",
          type: "message_update",
        })}\n`,
      );
    }
    child.emitStdout(
      `${JSON.stringify({
        finishReason: "backpressure",
        message: "Tomcat dropped intermediate deltas to keep up.",
        sessionId: "s1",
        type: "llm_notice",
      })}\n`,
    );
    child.emitStdout(
      `${JSON.stringify({
        error: null,
        messages: [],
        sessionId: "s1",
        type: "agent_end",
      })}\n`,
    );

    await renderQueue;
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(progresses).toContain(
      "Tomcat dropped intermediate deltas to keep up.",
    );
    expect(markdowns.join("")).toBe("abcd");
    expect(sawAgentEnd).toBe(true);
  });
});
