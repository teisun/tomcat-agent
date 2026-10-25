import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../provider";
import { isWebviewIntent, type WebviewIntent } from "../protocol";
import type { ServeEvent } from "../../../serveClient/wire";
import { recoveryClock } from "./errorRecoveryClock";

const providers: TomcatWebviewViewProvider[] = [];
afterEach(() => { providers.splice(0).forEach(p => p.dispose()); vi.restoreAllMocks(); });
const flush = () => new Promise<void>(resolve => setImmediate(resolve));
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function history(sessionId: string, action: "retry" | "resume" = "retry") {
  return { sessionId, messages: [
    { type: "message", id: "user-1", message: { role: "user", content: "original request", superseded: action === "retry", turn_failed: true } },
    ...(action === "resume" ? [
      { type: "message", id: "assistant-1", message: { role: "assistant", content: null, tool_calls: [{ id: "read-1", name: "read" }] } },
      { type: "message", id: "tool-1", message: { role: "tool", content: "completed", tool_call_id: "read-1" } },
    ] : []),
    { type: "error", id: "error-1", summary: "Authentication failed", detail: "HTTP 401" },
  ] };
}
function fixture(action: "retry" | "resume" = "retry") {
  const time = recoveryClock();
  let listener!: (event: ServeEvent) => void;
  const retry = vi.fn().mockResolvedValue(undefined);
  const resume = vi.fn().mockResolvedValue(undefined);
  const getMessages = vi.fn(async (id: string) => history(id, action));
  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/workspace/extension"),
    extensionMode: vscode.ExtensionMode.Development,
    getDefaultCwd: () => "/workspace", ide: {} as never,
    initialize: async () => ({ sessionId: "s1", capabilities: [] } as never),
    messenger: { onEvent: (callback: typeof listener) => { listener = callback; return { dispose() {} }; } } as never,
    sessionRouter: { retry, resume, getMessages } as never,
    errorRecoveryClock: time.clock,
  });
  providers.push(provider);
  const host = provider as any;
  host.stateStore.setActiveSession("s1");
  host.stateStore.hydrateHistory("s1", history("s1", action));
  vi.spyOn(host, "ensureInitialized").mockResolvedValue({ sessionId: "s1" });
  const select = vi.spyOn(host, "ensureWebviewSession").mockImplementation(async (id: unknown) => { host.stateStore.setActiveSession(id); return id; });
  vi.spyOn(host, "refreshSessionState").mockResolvedValue(undefined);
  vi.spyOn(host, "refreshSessions").mockResolvedValue(undefined);
  const emit = async (event: object) => { listener(event as ServeEvent); await host.serveEventQueue; };
  const failed = async (id = "s1") => {
    await emit({ type: "agent_start", sessionId: id });
    await emit({ type: "agent_end", sessionId: id, messages: [], error: "HTTP 401" });
    await emit({ type: "agent_idle", sessionId: id, outcome: "failed" });
  };
  const projection = (id = "s1") => provider.currentState().sessionViews[id]?.errorRecovery;
  const card = () => provider.currentState().sessionViews.s1.timeline.find(item => item.type === "message" && item.id === "error-1");
  return { provider, host, time, retry, resume, getMessages, emit, failed, projection, card, select };
}

for (const action of ["retry", "resume"] as const) describe(`${action} countdown`, () => {
  it("waits 300000 ms, uses only the original router command, and never selects the background session", async () => {
    const f = fixture(action);
    await f.failed();
    expect(f.projection()).toEqual({ errorId: "error-1", dueAt: f.time.clock.now() + 300_000 });
    f.host.stateStore.setActiveSession("s2");
    f.time.advance(299_999); await flush();
    expect(f.retry).not.toHaveBeenCalled(); expect(f.resume).not.toHaveBeenCalled();
    f.time.advance(1); await flush();
    if (action === "retry") { expect(f.retry).toHaveBeenCalledExactlyOnceWith("s1", "user-1"); expect(f.resume).not.toHaveBeenCalled(); }
    else { expect(f.resume).toHaveBeenCalledExactlyOnceWith("s1"); expect(f.retry).not.toHaveBeenCalled(); }
    expect(f.provider.currentState().activeSessionId).toBe("s2");
    expect(f.select).not.toHaveBeenCalled();
    expect(f.projection()).toBeUndefined();
    expect(f.time.pending()).toBe(0);
  });

  it("cancels only this wait, retains the manual button, and rejects stale cancel intents", async () => {
    const f = fixture(action); await f.failed();
    const wait = f.projection()!;
    await f.provider.dispatchTestIntent({ type: "cancelErrorRecovery", messageId: "stale", data: { sessionId: "s1", ...wait, dueAt: wait.dueAt - 1 } });
    expect(f.projection()).toEqual(wait);
    const [oldCallback] = f.time.callbacks();
    await f.provider.dispatchTestIntent({ type: "cancelErrorRecovery", messageId: "cancel", data: { sessionId: "s1", ...wait } });
    oldCallback(); f.time.advance(300_000); await flush();
    expect(f.retry).not.toHaveBeenCalled(); expect(f.resume).not.toHaveBeenCalled();
    expect(f.card()).toMatchObject({ recoveryAction: action });
    await f.provider.dispatchTestIntent({ type: "recoverErrorTurn", messageId: "manual", data: { sessionId: "s1", errorId: "error-1", action } });
    expect(action === "retry" ? f.retry : f.resume).toHaveBeenCalledTimes(1);
  });

  it("rearms after a fresh failure that reuses the same durable error ID", async () => {
    const f = fixture(action); await f.failed();
    const old = f.projection()!;
    f.time.advance(300_000); await flush();
    await f.failed();
    expect(f.card()).toMatchObject({ recoveryAction: action });
    expect(f.projection()).toEqual({ errorId: old.errorId, dueAt: old.dueAt + 300_000 });
    await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" });
    await f.host.refreshSessionHistory("s1");
    expect(f.time.pending()).toBe(1);
    f.time.advance(300_000); await flush();
    expect(action === "retry" ? f.retry : f.resume).toHaveBeenCalledTimes(2);
  });
});

describe("recovery lifecycle and ownership", () => {
  it.each(["upstream network timeout", "authentication_error", "insufficient credits", "content_filter"])("schedules an eligible task error without a second category filter: %s", async summary => {
    const f = fixture();
    const next = history("s1");
    next.messages[next.messages.length - 1] = { type: "error", id: "error-1", summary, detail: summary };
    f.getMessages.mockResolvedValueOnce(next);
    await f.failed();
    expect(f.card()).toMatchObject({ text: summary, recoveryAction: "retry" });
    expect(f.projection()?.dueAt).toBe(f.time.clock.now() + 300_000);
    f.time.advance(300_000); await flush();
    expect(f.retry).toHaveBeenCalledExactlyOnceWith("s1", "user-1");
  });

  it("reopens manual Resume after a same-ID failure even when the previous automatic wait was cancelled", async () => {
    const f = fixture("resume"); await f.failed();
    await f.provider.dispatchTestIntent({ type: "cancelErrorRecovery", messageId: "cancel", data: { sessionId: "s1", ...f.projection()! } });
    const click = () => f.provider.dispatchTestIntent({ type: "recoverErrorTurn", messageId: "manual", data: { sessionId: "s1", errorId: "error-1", action: "resume" } });
    await click(); expect(f.resume).toHaveBeenCalledTimes(1);
    expect(f.card()).not.toHaveProperty("recoveryAction", "resume");
    f.time.advance(1); await f.failed();
    expect(f.card()).toMatchObject({ recoveryAction: "resume" });
    expect(f.projection()?.errorId).toBe("error-1");
    await click(); expect(f.resume).toHaveBeenCalledTimes(2);
  });

  it("does not schedule a failed observed session", async () => {
    const f = fixture(); f.host.stateStore.view().sessionViews.s1.access = "observe";
    await f.failed(); f.time.advance(300_000); await flush();
    expect(f.projection()).toBeUndefined(); expect(f.retry).not.toHaveBeenCalled();
  });

  it("never schedules from old history, interrupted runs or ordinary error notifications", async () => {
    const f = fixture();
    await f.host.refreshSessionHistory("s1");
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [], error: "interrupted" });
    await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "interrupted" });
    expect(f.projection()).toBeUndefined(); expect(f.time.pending()).toBe(0);
  });

  it("manual click, double click and a stale timer callback claim the button once", async () => {
    const f = fixture(); await f.failed(); const [callback] = f.time.callbacks();
    const ack = deferred<void>(); f.retry.mockReturnValue(ack.promise);
    const click = () => f.provider.dispatchTestIntent({ type: "recoverErrorTurn", messageId: "manual", data: { sessionId: "s1", errorId: "error-1", action: "retry" } });
    const first = click(); const second = click(); callback(); await flush();
    expect(f.retry).toHaveBeenCalledTimes(1); ack.resolve(); await Promise.all([first, second]);
  });

  it.each(["busy", "command", "in-flight", "running", "question", "deleting", "observe", "missing", "changed-action"])("rechecks %s at the deadline without dispatching", async reason => {
    const f = fixture(); await f.failed();
    const view = f.host.stateStore.view().sessionViews.s1;
    if (reason === "busy") view.busy = true;
    if (reason === "command") view.commandPending = true;
    if (reason === "in-flight") vi.spyOn(f.host.messageQueue, "hasInFlight").mockReturnValue(true);
    if (reason === "running") vi.spyOn(f.host.messageQueue, "isRunning").mockReturnValue(true);
    if (reason === "question") f.host.pendingQuestions.set("pending", { sessionId: "s1", settled: false, resolve() {} });
    if (reason === "deleting") f.host.pendingDeletions.set("s1", {});
    if (reason === "observe") view.access = "observe";
    if (reason === "missing") view.timeline = [];
    if (reason === "changed-action") view.timeline.find((v: any) => v.id === "error-1").recoveryAction = "resume";
    f.time.advance(300_000); await flush();
    expect(f.retry).not.toHaveBeenCalled(); expect(f.resume).not.toHaveBeenCalled(); expect(f.projection()).toBeUndefined();
    f.host.pendingQuestions.clear();
  });

  const takeover = [
    { type: "prompt", data: { text: "new request" } },
    { type: "steer", data: { text: "new direction" } },
    { type: "retryUserMessage", data: { messageId: "user-1" } },
    { type: "recoverErrorTurn", data: { errorId: "error-1", action: "retry" } },
    { type: "runSlashCommand", data: { text: "/compact" } },
    { type: "compact", data: {} },
    { type: "rewindAndResend", data: { messageId: "user-1", text: "revised request" } },
    { type: "restoreSessionFiles", data: { sourceTurnId: "turn-1", paths: ["file.ts"], requestId: "restore" } },
    { type: "closeSession", data: {} },
    { type: "deleteSession", data: {} },
    { type: "queueAction", data: { userMessageId: "user-1", action: "send" } },
    { type: "setPlanMode", data: { planId: "plan-1", action: "build" } },
  ];
  for (const phase of ["pending-history", "countdown"]) it.each(takeover)(`${phase}: $type takes over before dispatch or asynchronous preparation`, async intent => {
    const f = fixture();
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [], error: "failed" });
    if (phase === "countdown") await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" });
    const callbacks = f.time.callbacks();
    // A blocked session exits immediately AFTER the takeover hook; later unblocking must not revive it.
    f.host.pendingDeletions.set("s1", {});
    await f.provider.dispatchTestIntent({ ...intent, messageId: "takeover", data: { ...intent.data, sessionId: "s1" } } as never);
    f.host.pendingDeletions.delete("s1");
    await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" });
    await f.host.refreshSessionHistory("s1");
    callbacks.forEach(callback => callback()); f.time.advance(300_000); await flush();
    expect(f.getMessages).toHaveBeenCalled(); // Cancelling automation must NOT cancel error-card refresh.
    expect(f.projection()).toBeUndefined(); expect(f.retry).not.toHaveBeenCalled();
  });

  it.each([
    { type: "syncComposerDraft", data: {} },
    { type: "attachFiles", data: {} },
    { type: "previewRewind", data: { messageId: "user-1" } },
    { type: "switchSession", data: {} },
    { type: "keepSessionFiles", data: { sourceTurnId: "turn-1", requestId: "keep" } },
    { type: "setPlanMode", data: { action: "enter" } },
  ])("$type does not take over the wait", async intent => {
    const f = fixture(); await f.failed(); const before = f.projection();
    f.host.pendingDeletions.set("s1", {});
    await f.provider.dispatchTestIntent({ ...intent, messageId: "inspect", data: { ...intent.data, sessionId: "s1" } } as never);
    f.host.pendingDeletions.delete("s1");
    expect(f.projection()).toEqual(before);
  });

  it.each(["retry", "resume"] as const)("external execution cancels %s even if it finishes before the deadline", async action => {
    const f = fixture(action); await f.failed();
    const [callback] = f.time.callbacks();
    await f.emit({ type: "agent_start", sessionId: "s1" });
    expect(f.projection()).toBeUndefined(); expect(f.time.pending()).toBe(0);
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [] });
    await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "completed" });
    expect(f.provider.currentState().sessionViews.s1.busy).toBe(false);
    callback(); f.time.advance(300_000); await flush();
    expect(f.retry).not.toHaveBeenCalled(); expect(f.resume).not.toHaveBeenCalled();
  });

  it.each(["pending-history", "countdown"])("workspace folder changes preserve %s and error-card refresh", async phase => {
    const f = fixture();
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [], error: "failed" });
    if (phase === "countdown") await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" });
    const before = f.projection();
    f.host.handleWorkspaceFolderChange();
    expect(f.projection()).toEqual(before);
    if (phase === "pending-history") await f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" });
    expect(f.getMessages).toHaveBeenCalledExactlyOnceWith("s1", expect.any(Object));
    expect(f.card()).toMatchObject({ recoveryAction: "retry" });
    expect(f.projection()).toBeDefined();
    f.time.advance(300_000); await flush();
    expect(f.retry).toHaveBeenCalledExactlyOnceWith("s1", "user-1");
  });

  it.each(["disconnect", "reconnect", "dispose"])("cleans all waits on %s and never recreates from history", async reason => {
    const f = fixture(); await f.failed(); await f.failed("s2");
    const callbacks = f.time.callbacks();
    if (reason === "disconnect") await f.provider.setServeConnectionState("failed");
    if (reason === "reconnect") f.provider.prepareForServeReconnect();
    if (reason === "dispose") f.provider.dispose();
    callbacks.forEach(callback => callback()); f.time.advance(300_000); await flush();
    expect(f.time.pending()).toBe(0); expect(f.retry).not.toHaveBeenCalled();
  });

  it("a cancelled session does not cancel a second session", async () => {
    const f = fixture(); await f.failed(); await f.failed("s2");
    await f.provider.dispatchTestIntent({ type: "cancelErrorRecovery", messageId: "cancel", data: { sessionId: "s1", ...f.projection()! } });
    f.time.advance(300_000); await flush();
    expect(f.retry).toHaveBeenCalledExactlyOnceWith("s2", "user-1");
  });

  it.each(["reject", "wrong-session"])("does not use stale history after %s", async kind => {
    const f = fixture();
    if (kind === "reject") f.getMessages.mockRejectedValueOnce(new Error("offline"));
    else f.getMessages.mockResolvedValueOnce(history("wrong"));
    await f.failed(); await f.host.refreshSessionHistory("s1");
    expect(f.projection()).toBeUndefined();
  });

  it("only the winning history fetch can schedule, and a stale failure cannot consume it", async () => {
    const f = fixture(); const old = deferred<ReturnType<typeof history>>();
    f.getMessages.mockReturnValueOnce(old.promise);
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [], error: "failed" });
    const idle = f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" }); await flush();
    await f.host.refreshSessionHistory("s1");
    const scheduled = f.projection(); expect(scheduled).toBeDefined();
    old.reject(new Error("stale fetch")); await idle;
    expect(f.projection()).toEqual(scheduled); expect(f.time.pending()).toBe(1);
  });

  it("a new prompt invalidates delayed history even if submission fails", async () => {
    const f = fixture(); const old = deferred<ReturnType<typeof history>>();
    f.getMessages.mockReturnValueOnce(old.promise);
    await f.emit({ type: "agent_end", sessionId: "s1", messages: [], error: "failed" });
    const idle = f.emit({ type: "agent_idle", sessionId: "s1", outcome: "failed" }); await flush();
    vi.mocked(f.host.ensureInitialized).mockRejectedValueOnce(new Error("submission failed"));
    await expect(f.provider.dispatchTestIntent({ type: "prompt", messageId: "new", data: { sessionId: "s1", text: "new request" } })).rejects.toThrow("submission failed");
    old.resolve(history("s1")); await idle;
    expect(f.card()).toMatchObject({ recoveryAction: "retry" }); expect(f.projection()).toBeUndefined();
  });

  it.each(["busy", "ack timeout", "nothing_to_resume"])("does not repeatedly send a rejected or uncertain command: %s", async error => {
    const f = fixture("resume"); f.resume.mockRejectedValue(new Error(error)); await f.failed();
    f.time.advance(300_000); await flush(); f.time.advance(300_000); await flush();
    expect(f.resume).toHaveBeenCalledTimes(1); expect(f.retry).not.toHaveBeenCalled();
    expect(f.card()).toMatchObject({ recoveryAction: "resume", recoveryError: expect.any(String) });
  });

  it.each(["retry", "resume"] as const)("manual %s dispatches during a pending command and displays the server rejection", async action => {
    const f = fixture(action); await f.failed();
    f.host.stateStore.view().sessionViews.s1.commandPending = true;
    const rpc = action === "retry" ? f.retry : f.resume;
    rpc.mockRejectedValueOnce(new Error("busy"));
    await f.provider.dispatchTestIntent({ type: "recoverErrorTurn", messageId: "manual", data: { sessionId: "s1", errorId: "error-1", action } });
    if (action === "retry") expect(f.retry).toHaveBeenCalledExactlyOnceWith("s1", "user-1");
    else expect(f.resume).toHaveBeenCalledExactlyOnceWith("s1");
    expect(f.card()).toMatchObject({ recoveryAction: action, recoveryError: expect.any(String) });
    expect(f.projection()).toBeUndefined();
    f.time.advance(300_000); await flush();
    expect(rpc).toHaveBeenCalledTimes(1);
  });

  it("an accepted RPC stays dismissed even when subsequent UI refresh throws", async () => {
    const f = fixture(); await f.failed();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(f.host.refreshSessions).mockRejectedValue(new Error("refresh failed"));
    f.time.advance(300_000); await flush();
    expect(f.retry).toHaveBeenCalledTimes(1); expect(f.card()).not.toHaveProperty("recoveryAction", "retry");
    await f.provider.dispatchTestIntent({ type: "recoverErrorTurn", messageId: "again", data: { sessionId: "s1", errorId: "error-1", action: "retry" } });
    expect(f.retry).toHaveBeenCalledTimes(1);
  });

  it("validates cancellation identity in the webview boundary", () => {
    const intent: WebviewIntent = { type: "cancelErrorRecovery", messageId: "cancel", data: { sessionId: "s1", errorId: "e1", dueAt: 123 } };
    expect(isWebviewIntent(intent)).toBe(true);
    expect(isWebviewIntent({ ...intent, data: { ...intent.data, dueAt: NaN } })).toBe(false);
    expect(isWebviewIntent({ ...intent, data: { ...intent.data, dueAt: "123" } })).toBe(false);
  });
});
