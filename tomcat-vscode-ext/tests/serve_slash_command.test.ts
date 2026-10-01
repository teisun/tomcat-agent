import { mkdir, writeFile } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { createRealServeMessenger, spawnScriptedOpenAiStreamServer, sseDelta, sseDone, sseFinish, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
describe("real Serve shared slash boundary", () => {
  it("uses the handshake whitelist, installs/uninstalls an agent Skill and never calls the model", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    try {
      const init = await initializeServe(runtime.messenger);
      const sessionId = init.sessionId!;
      const router = new SessionRouter(runtime.messenger, () => runtime.fixture.workspacePath);
      expect(init.capabilities).toContain("run_slash_command");
      expect(init.slashCommands?.map((command) => command.name)).toEqual(["reload", "install", "uninstall"]);
      const before = await runtime.messenger.request({type:"get_messages",sessionId});
      expect((await router.runSlashCommand(sessionId, "/reload")).ok).toBe(true);
      expect(await router.runSlashCommand(sessionId, "/unknown")).toMatchObject({ok:false});
      expect(await router.runSlashCommand(sessionId, "/install ./missing")).toMatchObject({ok:false,text:expect.stringContaining("用法")});
      const source = path.join(runtime.fixture.workspacePath, "source with space", "remote-slash-skill");
      await mkdir(source, {recursive:true});
      await writeFile(path.join(source, "SKILL.md"), "---\nname: remote-slash-skill\ndescription: Slash integration fixture\n---\nSay hello.\n");
      expect(await router.runSlashCommand(sessionId, `/install '${source}' agent`)).toMatchObject({ok:true});
      expect(await router.runSlashCommand(sessionId, "/uninstall remote-slash-skill agent")).toMatchObject({ok:true});
      const after = await runtime.messenger.request({type:"get_messages",sessionId});
      expect(after.payload).toEqual(before.payload);
      expect(server.capturedRequests()).toHaveLength(0);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 45_000);
  it("rejects a command while the same session is answering, without an additional model request", async () => {
    const server = await spawnScriptedOpenAiStreamServer([{parts:[sseDelta("running"), {...sseFinish("stop"),delayMs:1200},sseDone()]}]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    try {
      const init = await initializeServe(runtime.messenger);
      const sessionId = init.sessionId!;
      const router = new SessionRouter(runtime.messenger, () => runtime.fixture.workspacePath);
      const started = waitForEvent(runtime.messenger, (event) => event.type === "message_update" && event.sessionId === sessionId);
      const ended = waitForEvent(runtime.messenger, (event) => event.type === "agent_idle" && event.sessionId === sessionId);
      await runtime.messenger.request({type:"prompt",sessionId,text:"hold",params:{}});
      await started;
      await expect(router.runSlashCommand(sessionId, "/reload")).rejects.toThrow("busy");
      await ended;
      expect((await router.runSlashCommand(sessionId, "/reload")).ok).toBe(true);
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 45_000);
});
