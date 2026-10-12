import { describe, expect, it } from "vitest";

import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readSingleCommandLine(child: FakeChildProcess): Record<string, unknown> {
  return JSON.parse(child.readStdin().trim()) as Record<string, unknown>;
}

describe("TomcatMessenger context-window wrapper", () => {
  it("sends the set_context_window serve command", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
    });
    const pending = messenger.sendSetContextWindow("s1", "gpt-5.6", 1_000_000);
    const command = readSingleCommandLine(child);

    expect(command).toMatchObject({
      contextWindow: 1_000_000,
      model: "gpt-5.6",
      sessionId: "s1",
      type: "set_context_window",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: command.id,
        payload: { contextWindow: 1_000_000, model: "gpt-5.6", sessionId: "s1" },
        sessionId: "s1",
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pending).resolves.toMatchObject({ success: true });
    messenger.dispose();
  });
});
