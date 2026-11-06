import { vi } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../provider";
import type { ServeEvent } from "../../../serveClient/wire";
import type { SessionHistoryPayload } from "../../../serveClient/sessionRouter";
import { recoveryClock } from "./errorRecoveryClock";

export function recoveryHistory(sessionId: string, action: "retry" | "resume" = "retry") {
  return { sessionId, messages: [
    { type: "message", id: "user-1", message: { role: "user", content: "original request", superseded: action === "retry", turn_failed: true } },
    ...(action === "resume" ? [
      { type: "message", id: "assistant-1", message: { role: "assistant", content: null, tool_calls: [{ id: "read-1", name: "read" }] } },
      { type: "message", id: "tool-1", message: { role: "tool", content: "completed", tool_call_id: "read-1" } },
    ] : []),
    { type: "error", id: "error-1", summary: "Authentication failed", detail: "HTTP 401" },
  ] };
}

/** Real Provider/state/recovery path; only transport, initialization and refresh I/O are replaced. */
export function recoveryFixture(action: "retry" | "resume" = "retry") {
  const time = recoveryClock();
  let listener!: (event: ServeEvent) => void;
  const retry = vi.fn().mockResolvedValue(undefined);
  const resume = vi.fn().mockResolvedValue(undefined);
  const restoreCheckpoint = vi.fn();
  const getMessages = vi.fn(async (id: string): Promise<SessionHistoryPayload> => recoveryHistory(id, action));
  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/workspace/extension"),
    extensionMode: vscode.ExtensionMode.Development,
    getDefaultCwd: () => "/workspace", ide: {} as never,
    initialize: async () => ({ sessionId: "s1", capabilities: [] } as never),
    messenger: { onEvent: (callback: typeof listener) => { listener = callback; return { dispose() {} }; } } as never,
    sessionRouter: { retry, resume, getMessages, restoreCheckpoint,
      switchSession: async (sessionId: string) => ({ sessionId, access: "control", origin: null }),
    } as never,
    errorRecoveryClock: time.clock,
  });
  const host = provider as any;
  host.stateStore.setActiveSession("s1");
  host.stateStore.hydrateHistory("s1", recoveryHistory("s1", action));
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
  return { provider, host, time, retry, resume, getMessages, restoreCheckpoint, emit, failed, projection, card, select };
}
