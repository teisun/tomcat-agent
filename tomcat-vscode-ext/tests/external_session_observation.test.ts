import * as path from "node:path";
import { mkdir } from "node:fs/promises";
import { expect, it } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";
import { startAcpTestHost } from "../src/test/suite/support/acpTestHost";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { WebviewStateStore } from "../src/ui/webview/state";
import type { ServeEvent } from "../src/serveClient/wire";
import { ensureTomcatBinary, setupServeFixture, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, sseToolCall } from "./serveTestUtils";

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


it("answers an ACP question after the observer reconnects without granting task control", async () => {
  const question = { id: "choice", prompt: "Choose before continuing", options: [
    { id: "first", label: "First" }, { id: "second", label: "Second", recommended: true },
  ] };
  const prefix = "Waiting for the human choice.";
  const model = await spawnScriptedOpenAiStreamServer([
    { parts: [sseDelta(prefix), sseToolCall("human-choice", "ask_question", JSON.stringify({ questions: [question] })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("The human answer was received."), sseFinish("stop"), sseDone()] },
  ]);
  const fixture = await setupServeFixture(model.baseUrl);
  const executable = await ensureTomcatBinary();
  const host = startAcpTestHost(executable, fixture.workspacePath, fixture.env);
  const makeWindow = () => {
    const messenger = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: fixture.env });
    const router = new SessionRouter(messenger, () => fixture.workspacePath);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file(fixture.workspacePath), getDefaultCwd: () => fixture.workspacePath,
      messenger, sessionRouter: router, initialize: () => initializeServe(messenger), ide: {} as never,
    });
    let deliveries = 0;
    messenger.registerAskQuestionHandler((request, frame, context) => {
      deliveries++;
      return provider.askUser(request, frame.sessionId, context.signal);
    });
    const state = (provider as any).stateStore as WebviewStateStore;
    return { messenger, provider, get deliveries() { return deliveries; }, async open(sessionId: string) {
      state.applySessionOpen(await router.switchSession(sessionId));
      state.applySessionState(await router.getState(sessionId));
    } };
  };
  const first = makeWindow(), next = makeWindow();
  try {
    await initializeServe(first.messenger);
    await host.request("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await host.request<{ sessionId: string }>("session/new", { cwd: fixture.workspacePath, mcpServers: [] });
    const turn = host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "Ask the user then continue" }] });
    void turn.catch(() => undefined);
    await expect.poll(() => JSON.stringify(host.updates), { timeout: 15_000 }).toContain(prefix);
    await first.open(sessionId);
    const pending = (provider: TomcatWebviewViewProvider) => provider.currentState().sessionViews[sessionId]?.timeline.find(item => item.type === "approval" && item.live && !item.resolved);
    await expect.poll(() => pending(first.provider)).toBeDefined();
    expect(first.provider.currentState().sessionViews[sessionId].access).toBe("observe");
    await first.messenger.disposeAsync();
    expect((first.provider as any).finalizedQuestionResults.size).toBe(0);
    expect(model.capturedNonTitleRequests()).toHaveLength(1);
    await initializeServe(next.messenger);
    await next.open(sessionId);
    await expect.poll(() => pending(next.provider)).toBeDefined();
    // Reopening can replay the same route; it must not create another handler/resolver.
    await next.open(sessionId);
    expect(next.deliveries).toBe(1);
    const card = pending(next.provider);
    expect(card?.type).toBe("approval");
    if (card?.type !== "approval") throw new Error("Missing live question card");
    for (const command of [{ type: "interrupt" }, { type: "prompt", text: "must not send" }] as const) {
      expect(await next.messenger.request({ ...command, sessionId })).toMatchObject({ success: false, error: "read_only_session" });
    }
    await next.provider.dispatchTestIntent({ type: "answerQuestion", messageId: "choose-non-recommended", data: {
      sessionId, requestId: card.request.requestId, result: { outcome: "answered", cancelled: false,
        answers: [{ questionId: "choice", optionIds: ["first"], pickedRecommended: false }],
      },
    } });
    await expect(turn).resolves.toMatchObject({ stopReason: "end_turn" });
    await expect.poll(() => pending(next.provider)).toBeUndefined();
    const requests = model.capturedNonTitleRequests();
    expect(requests).toHaveLength(2);
    const body = JSON.parse(requests[1].split("\r\n\r\n")[1]);
    const tool = body.messages.find((message: { role: string; tool_call_id?: string }) => message.role === "tool" && message.tool_call_id === "human-choice");
    expect(JSON.parse(tool.content)).toMatchObject({ outcome: "answered", answers: [{ question_id: "choice", option_ids: ["first"], picked_recommended: false }] });
    expect(next.provider.currentState().sessionViews[sessionId].access).toBe("observe");
  } finally {
    first.provider.dispose(); next.provider.dispose();
    try { await Promise.all([first.messenger.disposeAsync(), next.messenger.disposeAsync(), host.close()]); }
    finally { try { await fixture.cleanup(); } finally { await model.close(); } }
  }
}, 120_000);
