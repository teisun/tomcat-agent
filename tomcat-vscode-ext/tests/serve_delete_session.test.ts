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
      await runtime.messenger.requestControl({
        type: "control_request", subtype: "initialize", requestId: "non-default-scope-init", payload: null,
      });
      const cwd = path.join(runtime.fixture.workspacePath, "different-project");
      await mkdir(cwd);
      const created = await runtime.messenger.request({ type: "new_session", params: { cwd, mode: "code" } });
      expect(created.success, JSON.stringify(created)).toBe(true);
      const id = created.sessionId!;
      const before = await runtime.messenger.request({ type: "list_sessions", scope: "disk" });
      expect((await runtime.messenger.request({ type: "close_session", sessionId: id })).success).toBe(true);
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
      const created = await send({ type: "new_session", params: { cwd: runtime.fixture.workspacePath } });
      expect(created.success, JSON.stringify(created)).toBe(true);
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
  // Independent-process occupancy remains covered by Rust's
  // independent_process_blocks_delete_and_process_exit_releases_occupancy.
  it("refuses observer deletion without taking control away from the owning connection", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    const other = new TomcatMessenger({ executable: await ensureTomcatBinary(), env: runtime.fixture.env, cwd: runtime.fixture.workspacePath, requestTimeoutMs: 10_000 });
    try {
      const first = await initializeServe(runtime.messenger);
      const second = await initializeServe(other);
      expect(second.sessionId).toBe(first.sessionId);
      const id = first.sessionId!;
      const refused = await other.request({ type: "delete_session", sessionId: id });
      expect(refused.success).toBe(false);
      expect(refused.error).toBe("read_only_session");
      const reopened = await runtime.messenger.request({ type: "switch_session", sessionId: id });
      expect(reopened.success, JSON.stringify(reopened)).toBe(true);
      expect((await other.request({ type: "close_session", sessionId: id })).success).toBe(true);
      expect((await runtime.messenger.request({ type: "delete_session", sessionId: id })).payload).toMatchObject({ warnings: [] });
    } finally { await other.disposeAsync(); await runtime.cleanup(); await server.close(); }
  }, 30_000);
});
