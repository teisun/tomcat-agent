import { mkdir } from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { createRealServeMessenger, ensureTomcatBinary, spawnScriptedOpenAiStreamServer, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
describe("real Serve session deletion", () => {
  it("keeps the trusted non-default scope when its final live runtime is closed", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    try {
      const initial = await initializeServe(runtime.messenger);
      const cwd = path.join(runtime.fixture.workspacePath, "different-project");
      await mkdir(cwd);
      const created = await runtime.messenger.request({ type: "new_session", params: { cwd, mode: "code" } });
      const id = created.sessionId!;
      const before = await runtime.messenger.request({ type: "list_sessions", scope: "disk" });
      await runtime.messenger.request({ type: "close_session", sessionId: initial.sessionId! });
      await runtime.messenger.request({ type: "close_session", sessionId: id });
      const after = await runtime.messenger.request({ type: "list_sessions", scope: "disk" });
      expect(after.payload).toEqual(before.payload);
      const reopened = await runtime.messenger.request({ type: "switch_session", sessionId: id });
      expect(reopened.success, JSON.stringify(reopened)).toBe(true);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 30_000);
  it("removes current/background/last sessions but close alone keeps history reopenable", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    const send = runtime.messenger.request.bind(runtime.messenger);
    try {
      const init = await initializeServe(runtime.messenger);
      expect(init.capabilities).toContain("delete_session");
      const a = init.sessionId!;
      const created = await send({ type: "new_session", params: {} });
      const b = created.sessionId!;
      expect((await send({ type: "close_session", sessionId: a })).success).toBe(true);
      expect((await send({ type: "switch_session", sessionId: a })).success).toBe(true);
      expect((await send({ type: "delete_session", sessionId: b })).payload).toMatchObject({ warnings: [] });
      const list = await send({ type: "list_sessions", scope: "disk" });
      expect(list.payload).toMatchObject({ activeSessionId: a, sessions: [{ sessionId: a }] });
      expect((await send({ type: "delete_session", sessionId: a })).payload).toMatchObject({ warnings: [] });
      expect((await send({ type: "list_sessions", scope: "disk" })).payload).toMatchObject({ sessions: [] });
      expect((await send({ type: "delete_session", sessionId: a })).payload).toMatchObject({ warnings: [] });
      expect((await send({ type: "switch_session", sessionId: a })).success).toBe(false);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 30_000);
  it("refuses another Serve process's occupied session and allows deletion after it closes", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    const other = new TomcatMessenger({ executable: await ensureTomcatBinary(), env: runtime.fixture.env, cwd: runtime.fixture.workspacePath, requestTimeoutMs: 10_000 });
    try {
      const first = await initializeServe(runtime.messenger);
      const second = await initializeServe(other);
      expect(second.sessionId).toBe(first.sessionId);
      const id = first.sessionId!;
      const refused = await runtime.messenger.request({ type: "delete_session", sessionId: id });
      expect(refused.error).toBe("session_in_use");
      const reopened = await runtime.messenger.request({ type: "switch_session", sessionId: id });
      expect(reopened.success, JSON.stringify(reopened)).toBe(true);
      expect((await other.request({ type: "close_session", sessionId: id })).success).toBe(true);
      expect((await runtime.messenger.request({ type: "delete_session", sessionId: id })).payload).toMatchObject({ warnings: [] });
    } finally { await other.disposeAsync(); await runtime.cleanup(); await server.close(); }
  }, 30_000);
});
