import { describe, expect, it } from "vitest";

import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readSingleCommandLine(child: FakeChildProcess): Record<string, unknown> {
  const written = child.readStdin().trim();
  expect(written.length).toBeGreaterThan(0);
  return JSON.parse(written);
}

describe("TomcatMessenger request/response routing", () => {
  it("returns a Reload receipt immediately and preserves context and generation", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({ executable: "tomcat", requestTimeoutMs: 50, spawnFactory: createSpawnFactory(child) });
    const context = { workspaceRoot: "/workspace/A" };
    const pending = messenger.sendReloadConnector("key", context);
    const command = readSingleCommandLine(child);
    expect(command).toMatchObject({ type: "reload_connector", configKey: "key", context });
    const payload = { configKey: "key", accepted: true, generation: "18446744073709551615", recoveryTimeoutMs: 111_250 };
    child.emitStdout(`${JSON.stringify({ id: command.id, type: "response", success: true, payload })}\n`);
    await expect(pending).resolves.toMatchObject({ success: true, payload });
    expect(child.readStdin().trim().split("\n")).toHaveLength(1);
  });

  it("routes project trust by root and keeps Add-and-Trust as one command", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({ executable: "tomcat", spawnFactory: createSpawnFactory(child) });
    const lookup = messenger.sendGetProjectTrust("/workspace/project/subdir");
    const query = readSingleCommandLine(child);
    expect(query).toMatchObject({ type: "get_project_trust", path: "/workspace/project/subdir" });
    child.emitStdout(`${JSON.stringify({ type: "response", id: query.id, success: true, payload: { projectRoot: "/workspace/project", trusted: false } })}\n`);
    await expect(lookup).resolves.toMatchObject({ payload: { projectRoot: "/workspace/project", trusted: false } });

    const mutation = messenger.sendTrustProject("/workspace/project");
    const trust = JSON.parse(child.readStdin().trim().split("\n").at(-1)!);
    expect(trust).toMatchObject({ type: "trust_project", projectRoot: "/workspace/project" });
    expect(trust).not.toHaveProperty("configKey");
    child.emitStdout(`${JSON.stringify({ type: "response", id: trust.id, success: true, payload: { projectRoot: "/workspace/project", trusted: true } })}\n`);
    await expect(mutation).resolves.toMatchObject({ payload: { trusted: true } });

    const add = messenger.sendAddConnector({ name: "browser", command: "node", args: [], scope: "workspace", trustProject: true, context: { workspaceRoot: "/workspace/project" } });
    const wire = JSON.parse(child.readStdin().trim().split("\n").at(-1)!);
    expect(wire).toMatchObject({ type: "add_connector", scope: "workspace", trustProject: true });
    expect(child.readStdin()).toBe("");
    child.emitStdout(`${JSON.stringify({ type: "response", id: wire.id, success: true, payload: { configSaved: true, connectionStarted: true } })}\n`);
    await expect(add).resolves.toMatchObject({ payload: { configSaved: true } });
  });

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
