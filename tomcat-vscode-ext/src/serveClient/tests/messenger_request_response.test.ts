import { describe, expect, it } from "vitest";

import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readSingleCommandLine(child: FakeChildProcess): Record<string, unknown> {
  const written = child.readStdin().trim();
  expect(written.length).toBeGreaterThan(0);
  return JSON.parse(written);
}

describe("TomcatMessenger request/response routing", () => {
  it("pairs response frames by id", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
    });

    const pending = messenger.request({
      text: "hello",
      type: "prompt",
    });
    const command = readSingleCommandLine(child);

    child.emitStdout(
      `${JSON.stringify({
        id: command.id,
        payload: { queued: false },
        sessionId: "s1",
        success: true,
        type: "response",
      })}\n`,
    );

    await expect(pending).resolves.toMatchObject({
      sessionId: "s1",
      success: true,
      type: "response",
    });
  });

  it("ignores unknown response ids and times out pending requests", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      requestTimeoutMs: 10,
      spawnFactory: createSpawnFactory(child),
    });

    const pending = messenger.request({
      text: "hello",
      type: "prompt",
    });
    const command = readSingleCommandLine(child);

    child.emitStdout(
      `${JSON.stringify({
        id: "different-id",
        success: true,
        type: "response",
      })}\n`,
    );

    await expect(pending).rejects.toThrow(
      `Timed out waiting for response ${String(command.id)}`,
    );
  });
});
