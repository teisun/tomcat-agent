import { describe, expect, it } from "vitest";

import { initializeServe } from "../initialize";
import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readLatestCommand(child: FakeChildProcess): Record<string, unknown> {
  return JSON.parse(child.readStdin().trim());
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
          serverVersion: "0.1.15",
          sessionId: "s-bootstrap",
        },
        requestId: command.requestId,
        sessionId: "s-bootstrap",
        type: "control_response",
      })}\n`,
    );

    await expect(pending).resolves.toEqual({
      capabilities: ["prompt", "ask_question"],
      protocolVersion: 1,
      serverVersion: "0.1.15",
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
    child.emitStdout(
      `${JSON.stringify({
        payload: {
          questions: [
            {
              id: "color",
              options: [
                { id: "blue", label: "Blue", recommended: true },
                { id: "green", label: "Green", recommended: false },
              ],
              prompt: "Pick a color",
            },
          ],
          requestId: "ask-1",
          responseEvent: "plan.ask_question.response.ask-1",
        },
        requestId: "ask-1",
        sessionId: "s1",
        subtype: "ask_question",
        type: "control_request",
      })}\n`,
    );

    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(readLatestCommand(child)).toMatchObject({
      requestId: "ask-1",
      sessionId: "s1",
      type: "control_response",
    });
  });
});
