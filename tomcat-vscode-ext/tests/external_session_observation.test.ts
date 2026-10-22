import * as path from "node:path";
import { mkdir } from "node:fs/promises";
import { expect, it } from "vitest";
import { startAcpTestHost } from "../src/test/suite/support/acpTestHost";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { WebviewStateStore } from "../src/ui/webview/state";
import type { ServeEvent } from "../src/serveClient/wire";
import { ensureTomcatBinary, setupServeFixture, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone } from "./serveTestUtils";

it("observes a real ACP task through the ordinary disk/open flow across reconnect without crossing projects", async () => {
  let release!: () => void;
  const hold = new Promise<void>(resolve => { release = resolve; });
  const prefix = "Generated before joining. ";
  const suffix = "Continued after reconnect.";
  const model = await spawnScriptedOpenAiStreamServer([{ parts: [sseDelta(prefix), { body: "", waitFor: hold }, sseDelta(suffix), sseFinish("stop"), sseDone()] }]);
  const fixture = await setupServeFixture(model.baseUrl);
  const executable = await ensureTomcatBinary();
  const foreignCwd = path.join(fixture.homePath, "another-project");
  await mkdir(foreignCwd);
  const makeWindow = (cwd: string) => new TomcatMessenger({ executable, cwd, env: fixture.env });
  const first = makeWindow(fixture.workspacePath);
  const reconnected = makeWindow(fixture.workspacePath);
  const foreignWindow = makeWindow(foreignCwd);
  const host = startAcpTestHost(executable, fixture.workspacePath, fixture.env);
  const nextHost = startAcpTestHost(executable, fixture.workspacePath, fixture.env);
  const store = new WebviewStateStore();
  first.onEvent(event => store.applyEvent(event));
  reconnected.onEvent(event => store.applyEvent(event));
  const foreignEvents: ServeEvent[] = [];
  foreignWindow.onEvent(event => foreignEvents.push(event));
  const initializeHost = (peer: typeof host) => peer.request("initialize", { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: "acp-observer-fixture", version: "1" } });
  try {
    const native = await initializeServe(first);
    await initializeHost(host);
    const created = await host.request<{ sessionId: string }>("session/new", { cwd: fixture.workspacePath, mcpServers: [] });
    const foreign = await host.request<{ sessionId: string }>("session/new", { cwd: foreignCwd, mcpServers: [] });
    await initializeServe(foreignWindow);
    const turn = host.request("session/prompt", { sessionId: created.sessionId, prompt: [{ type: "text", text: "Work in this project" }] });
    void turn.catch(() => undefined);
    await expect.poll(() => JSON.stringify(host.updates), { timeout: 15_000 }).toContain(prefix);
    const router = new SessionRouter(first, () => fixture.workspacePath);
    const list = await router.listSessions("disk");
    expect(list.sessions.map(row => row.sessionId).sort()).toEqual([native.sessionId, created.sessionId].sort());
    expect(list.sessions.find(row => row.sessionId === created.sessionId)).toMatchObject({ access: "observe", origin: { kind: "acp", clientName: "acp-observer-fixture" } });
    await expect(router.switchSession(foreign.sessionId)).rejects.toThrow("unknown_session");
    const open = async (connection: TomcatMessenger) => {
      const sessions = new SessionRouter(connection, () => fixture.workspacePath);
      store.applySessionOpen(await sessions.switchSession(created.sessionId));
      store.applySessionState(await sessions.getState(created.sessionId));
      store.hydrateHistory(created.sessionId, await sessions.getMessages(created.sessionId));
    };
    await open(first);
    const assistant = () => store.snapshot().sessionViews[created.sessionId]?.timeline.filter(item => item.type === "message" && item.kind === "assistant");
    await expect.poll(() => assistant()).toEqual([expect.objectContaining({ text: prefix })]);
    expect(store.snapshot().sessionViews[created.sessionId]).toMatchObject({ access: "observe", ownedByThisFrontend: false, busy: true });
    for (const command of [{ type: "prompt", text: "must not send" }, { type: "interrupt" }, { type: "delete_session" }] as const) {
      expect(await first.request({ ...command, sessionId: created.sessionId })).toMatchObject({ success: false, error: "read_only_session" });
    }
    expect(await first.request({ type: "close_session", sessionId: created.sessionId })).toMatchObject({ success: true });
    await first.disposeAsync();
    await initializeServe(reconnected);
    await open(reconnected);
    await expect.poll(() => assistant()).toEqual([expect.objectContaining({ text: prefix })]);
    release();
    await expect(turn).resolves.toMatchObject({ stopReason: "end_turn" });
    await expect.poll(() => assistant()).toEqual([expect.objectContaining({ text: prefix + suffix })]);
    // Reopening after completion overlaps persisted history with the replayed tail.
    await open(reconnected);
    expect(assistant()).toEqual([expect.objectContaining({ text: prefix + suffix })]);
    const after = await new SessionRouter(reconnected, () => fixture.workspacePath).listSessions("disk");
    expect(after.sessionKey).toBe(list.sessionKey);
    expect(after.sessions).toHaveLength(2);
    expect(foreignEvents.some(event => event.sessionId === created.sessionId)).toBe(false);
    const otherList = await new SessionRouter(foreignWindow, () => foreignCwd).listSessions("disk");
    expect(otherList.sessions.some(row => row.sessionId === created.sessionId)).toBe(false);
    await host.close();
    await open(reconnected);
    expect(store.snapshot().sessionViews[created.sessionId].access).toBe("observe");
    await initializeHost(nextHost);
    await expect(nextHost.request("session/load", { sessionId: created.sessionId, cwd: fixture.workspacePath, mcpServers: [] })).resolves.toBeDefined();
    expect(model.capturedNonTitleRequests()).toHaveLength(1);
  } finally {
    release();
    try { await Promise.all([host.close(), nextHost.close(), first.disposeAsync(), reconnected.disposeAsync(), foreignWindow.disposeAsync()]); }
    finally { try { await fixture.cleanup(); } finally { await model.close(); } }
  }
}, 120_000);
