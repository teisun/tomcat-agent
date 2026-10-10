import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import type { ComposerDraftStore } from "../../../shared/composerDraft";
import { TomcatWebviewViewProvider } from "../provider";
import { translate } from "../../../shared/i18n";

const providers: TomcatWebviewViewProvider[] = [];
let sequence = 0;
afterEach(() => { for (const provider of providers.splice(0)) provider.dispose(); });

async function fixture(ids = ["a", "b"]) {
  const records = new Map(ids.map((id, index) => [id, { sessionId: id, title: id, updatedAt: index + 1, busy: false }]));
  const control = { active: ids[0] as string | null, scope: "scope-A", failList: false, hideAll: false, createFails: false, next: 0 };
  const listeners = new Set<(event: unknown) => void>();
  const storage = new Map<string, unknown>();
  const pinStorage = { get: <T>(key: string, fallback?: T): T => (storage.get(key) ?? fallback) as T,
    update: vi.fn(async (key: string, value: unknown) => { storage.set(key, value); }) };
  const erase = (id: string) => { records.delete(id); if (control.active === id) control.active = [...records.keys()][0] ?? null; };
  const request = vi.fn(async (command: { type: string; sessionId?: string }) => {
    if (command.type === "delete_session") { erase(command.sessionId!); return { type: "response", success: true, payload: { warnings: [] as string[] } }; }
    return { type: "response", success: true, payload: {} };
  });
  const router = {
    listSessions: vi.fn(async (scope = "live") => {
      if (control.failList) throw new Error("disconnected");
      return { scope, sessionKey: control.scope, activeSessionId: control.active,
        sessions: control.hideAll ? [] : [...records.values()].map(s => ({ ...s, isCurrent: s.sessionId === control.active })) };
    }),
    switchSession: vi.fn(async (id: string) => { if (!records.has(id)) throw new Error("unknown_session"); control.active = id; return id; }),
    newSession: vi.fn(async () => {
      if (control.createFails) throw new Error("creation failed");
      const id = `new-${++control.next}`;
      records.set(id, { sessionId: id, title: id, updatedAt: Date.now(), busy: false }); control.active = id; return id;
    }),
    getState: vi.fn(async (id: string) => ({ sessionId: id, sessionKey: control.scope, busy: false, agentMode: "chat", model: "model", thinkingLevel: "medium" })),
    getMessages: vi.fn(async (id: string) => ({ sessionId: id, messages: [], hasMore: false })),
    listCheckpoints: vi.fn(async (id: string) => ({ sessionId: id, checkpoints: [] })),
  };
  const root = vscode.Uri.file(`/delete-test-${++sequence}`);
  const provider = new TomcatWebviewViewProvider({
    extensionUri: root, getDefaultCwd: () => "/workspace", ide: {} as never, sessionPinStorage: pinStorage,
    draftStorage: { storageUri: root, globalStorageUri: root },
    initialize: async () => ({ attachmentRoot: null, capabilities: ["delete_session", "message_queue"], protocolVersion: 2, serverVersion: "test", sessionId: ids[0] ?? null }),
    messenger: { request, onEvent: (listener: (event: unknown) => void) => { listeners.add(listener); return { dispose: () => listeners.delete(listener) }; } } as never,
    sessionRouter: router as never,
  });
  providers.push(provider);
  await provider.dispatchTestIntent({ type: "ready", messageId: "ready" });
  const drafts = (provider as unknown as { draftStore: ComposerDraftStore }).draftStore;
  async function seed(id: string) {
    await provider.dispatchTestIntent({ type: "syncComposerDraft", messageId: `draft-${id}`, data: { sessionId: id, text: "keep me", segments: [{ type: "text", text: "keep me" }] } });
    await drafts.flush(id);
    await provider.dispatchTestIntent({ type: "setSessionPinned", messageId: `pin-${id}`, data: { sessionId: id, pinned: true } });
  }
  return { provider, control, request, router, records, drafts, erase, seed, pinStorage,
    delete: (id: string) => provider.dispatchTestIntent({ type: "deleteSession", messageId: `delete-${id}`, data: { sessionId: id } }),
    emit: (event: unknown) => { for (const listener of listeners) listener(event); },
    draftUri: (id: string) => vscode.Uri.joinPath(root, "composer-drafts", `${id}.json`),
  };
}

describe("Host session deletion", () => {
  it("reports a failed pin save without compensation writes", async () => {
    const f = await fixture();
    const update = f.pinStorage.update.getMockImplementation()!;
    f.pinStorage.update.mockImplementationOnce(async (key, value) => {
      await update(key, value);
      throw new Error("disk error");
    });
    const report = vi.spyOn(vscode.window, "showErrorMessage").mockResolvedValue(undefined);
    try {
      await f.provider.dispatchTestIntent({ type: "setSessionPinned", messageId: "failed-pin", data: { sessionId: "a", pinned: true } });
      expect(report).toHaveBeenCalledWith(translate("en", "session.pin.failed", { detail: "disk error" }));
      expect(f.pinStorage.update).toHaveBeenCalledTimes(1);
    } finally { report.mockRestore(); }
  });
  it("does not block deletion behind a pending pin save or revive the deleted row", async () => {
    const f = await fixture();
    const update = f.pinStorage.update.getMockImplementation()!;
    let release!: () => void;
    f.pinStorage.update.mockImplementationOnce((key, value) => {
      void update(key, value);
      return new Promise<void>(resolve => { release = resolve; });
    });
    const pin = f.provider.dispatchTestIntent({ type: "setSessionPinned", messageId: "pending-pin", data: { sessionId: "b", pinned: true } });
    try {
      await f.delete("b");
      expect(f.request).toHaveBeenCalledWith(expect.objectContaining({ type: "delete_session", sessionId: "b" }));
    } finally {
      release();
      await pin;
    }
    expect(f.provider.currentState().sessions.some(s => s.sessionId === "b")).toBe(false);
  });
  it("cleans a background session without switching, then drops late events and draft updates", async () => {
    const f = await fixture(); await f.seed("b");
    const switches = f.router.switchSession.mock.calls.length;
    await f.delete("b");
    expect(f.provider.currentState().activeSessionId).toBe("a");
    expect(f.router.switchSession).toHaveBeenCalledTimes(switches);
    expect(f.provider.currentState().sessionViews.b).toBeUndefined();
    await expect(vscode.workspace.fs.stat(f.draftUri("b"))).rejects.toThrow();
    f.emit({ type: "agent_start", sessionId: "b" });
    await new Promise(resolve => setTimeout(resolve, 0));
    await f.provider.dispatchTestIntent({ type: "syncComposerDraft", messageId: "late-draft", data: { sessionId: "b", text: "late", segments: [] } });
    expect(f.provider.currentState().sessionViews.b).toBeUndefined();
    expect(f.drafts.peek("b").text).toBe("");
  });
  it("chooses disk current rather than pin order and creates an empty session after the last deletion", async () => {
    const f = await fixture(); await f.seed("a");
    await f.delete("a");
    expect(f.provider.currentState().activeSessionId).toBe("b");
    await f.delete("b");
    expect(f.provider.currentState().activeSessionId).toBe("new-1");
    expect(f.router.newSession).toHaveBeenCalledTimes(1);
  });
  it("restores delivery and reopens after an explicit refusal without touching draft or pin", async () => {
    const f = await fixture(); await f.seed("a");
    f.request.mockResolvedValueOnce({ type: "response", success: false, error: "session_in_use" } as never);
    await f.delete("a");
    expect(f.drafts.peek("a").text).toBe("keep me");
    expect(f.provider.currentState().sessions.find(s => s.sessionId === "a")?.isPinned).toBe(true);
    expect(f.provider.currentState().sessionViews.a.deleting).toBe(false);
    expect(f.provider.currentState().sessionActionFeedback?.code).toBe("session_in_use");
    expect(f.router.switchSession).toHaveBeenLastCalledWith("a");
  });
  it("recovers a committed deletion with a lost response only from a same-scope disk list", async () => {
    const f = await fixture(); await f.seed("a");
    f.request.mockImplementationOnce(async () => { f.erase("a"); throw new Error("timeout after commit"); });
    await f.delete("a");
    expect(f.provider.currentState().activeSessionId).toBe("b");
    expect(f.drafts.peek("a").text).toBe("");
    expect(f.router.listSessions.mock.calls.every(([scope]) => scope === "disk")).toBe(true);
  });
  it.each(["failed-read", "wrong-scope", "malformed-receipt"])("retains and freezes uncertain data after %s, then resumes when present", async (failure) => {
    const f = await fixture(); await f.seed("a");
    f.request.mockImplementationOnce(async () => {
      if (failure === "malformed-receipt") {
        f.control.failList = true;
        return { type: "response", success: true, payload: {} } as never;
      }
      if (failure === "failed-read") f.control.failList = true;
      else { f.control.scope = "scope-B"; f.control.hideAll = true; }
      throw new Error("lost response");
    });
    await f.delete("a");
    expect(f.provider.currentState().sessionViews.a.deleting).toBe(true);
    expect(f.drafts.peek("a").text).toBe("keep me");
    const count = f.request.mock.calls.length;
    await f.provider.dispatchTestIntent({ type: "prompt", messageId: "blocked", data: { sessionId: "a", text: "do not send" } });
    expect(f.request).toHaveBeenCalledTimes(count);
    expect(f.provider.currentState().sessions.find(s => s.sessionId === "a")?.isPinned).toBe(true);
    f.control.failList = false; f.control.scope = "scope-A"; f.control.hideAll = false;
    await f.provider.refreshAfterServeRestart();
    expect(f.provider.currentState().sessionViews.a.deleting).toBe(false);
    expect(f.drafts.peek("a").text).toBe("keep me");
  });
  it("reports a strict draft cleanup failure as a warning after the backend commit", async () => {
    const f = await fixture(); await f.seed("b");
    const original = vscode.workspace.fs.delete.bind(vscode.workspace.fs);
    const deletion = vi.spyOn(vscode.workspace.fs, "delete").mockImplementation(async (uri, options) => {
      if (uri.path === f.draftUri("b").path) throw new Error("read-only draft");
      await original(uri, options);
    });
    try {
      await f.delete("b");
      expect(f.provider.currentState().sessions.some(s => s.sessionId === "b")).toBe(false);
      expect(f.provider.currentState().sessionActionFeedback).toMatchObject({ code: "partial_cleanup", detail: expect.stringContaining("read-only draft") });
    } finally { deletion.mockRestore(); }
  });
  it("keeps cleanup and fallback details for the same deletion without leaking to the next request", async () => {
    const f = await fixture(); await f.seed("a");
    f.request.mockImplementationOnce(async () => {
      f.erase("a");
      return { type: "response", success: true, payload: { warnings: ["lease cleanup failed"] } };
    });
    f.router.switchSession.mockRejectedValueOnce(new Error("open failed"));
    await f.delete("a");
    expect(f.provider.currentState().sessions.some(s => s.sessionId === "a")).toBe(false);
    expect(f.drafts.peek("a").text).toBe("");
    expect(f.provider.currentState().sessionActionFeedback).toMatchObject({
      id: "delete-a", code: "fallback_failed", detail: expect.stringContaining("lease cleanup failed"),
    });
    expect(f.provider.currentState().sessionActionFeedback?.detail).toContain("open failed");
    f.control.createFails = true;
    await f.delete("b");
    expect(f.provider.currentState().sessionActionFeedback).toMatchObject({
      id: "delete-b", code: "fallback_failed", detail: expect.stringContaining("creation failed"),
    });
    expect(f.provider.currentState().sessionActionFeedback?.detail).not.toContain("lease cleanup failed");
    expect(f.provider.currentState().sessionActionFeedback?.detail).not.toContain("open failed");
  });
  it("reports cleanup/fallback failures without undoing a confirmed deletion", async () => {
    const f = await fixture(["a"]); await f.seed("a");
    f.control.createFails = true;
    await f.delete("a");
    expect(f.provider.currentState().activeSessionId).toBeNull();
    expect(f.provider.currentState().sessions).toEqual([]);
    expect(f.provider.currentState().sessionActionFeedback?.code).toBe("fallback_failed");
    expect(f.drafts.peek("a").text).toBe("");
  });
});
