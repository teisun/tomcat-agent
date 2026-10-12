import { describe, expect, it } from "vitest";
import { TomcatMessenger } from "../TomcatMessenger";
import { SPEEDS } from "../../shared/modelSpeed";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

describe("TomcatMessenger speed wrapper", () => {
  it.each(SPEEDS)("sends exact set_speed parameters for %s", async (speed) => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({ executable: "tomcat", spawnFactory: createSpawnFactory(child) });
    const pending = messenger.sendSetSpeed("s1", "relay/model", speed);
    const command = JSON.parse(child.readStdin().trim()) as Record<string, unknown>;
    expect(command).toEqual({ id: expect.any(String), type: "set_speed", sessionId: "s1", model: "relay/model", speed });
    child.emitStdout(`${JSON.stringify({ id: command.id, type: "response", success: true, sessionId: "s1", payload: { model: "relay/model", speed } })}\n`);
    await expect(pending).resolves.toMatchObject({ success: true, payload: { speed } });
    messenger.dispose();
  });
});
