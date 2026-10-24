import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../provider";
import type { HostToWebviewFrame } from "../protocol";
import type { WebviewStateStore } from "../state";
import { en } from "../../../shared/i18n/en";

function makeView() {
  const disposed = new Set<() => void>();
  const messages = new Set<(message: unknown) => void>();
  let lastMessageListener: (message: unknown) => void = () => undefined;
  const postMessage = vi.fn<(frame: HostToWebviewFrame) => Promise<boolean>>().mockResolvedValue(true);
  const view = {
    visible: true,
    onDidDispose: (listener: () => void) => {
      disposed.add(listener);
      return new vscode.Disposable(() => { disposed.delete(listener); });
    },
    onDidChangeVisibility: () => new vscode.Disposable(() => undefined),
    show() {},
    webview: {
      asWebviewUri: (uri: vscode.Uri) => uri,
      cspSource: "vscode-test-webview", html: "", options: {}, postMessage,
      onDidReceiveMessage: (listener: (message: unknown) => void) => {
        lastMessageListener = listener;
        messages.add(listener);
        return new vscode.Disposable(() => { messages.delete(listener); });
      },
    },
  };
  return {
    view: view as unknown as vscode.WebviewView,
    setVisible: (visible: boolean) => { view.visible = visible; },
    postMessage,
    dispose: () => { for (const listener of [...disposed]) listener(); },
    pong: () => { for (const listener of messages) listener({ type: "pong", messageId: "pong" }); },
    stalePong: () => lastMessageListener({ type: "pong", messageId: "stale-pong" }),
  };
}

const providers: TomcatWebviewViewProvider[] = [];
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-24T00:00:00Z"));
  vi.spyOn(console, "warn").mockImplementation(() => undefined);
});
afterEach(() => {
  for (const provider of providers.splice(0)) provider.dispose();
  vi.useRealTimers();
});

function setup(extensionMode = vscode.ExtensionMode.Production) {
  const eventDispose = vi.fn();
  const request = vi.fn();
  const provider = new TomcatWebviewViewProvider({
    extensionMode,
    extensionUri: vscode.Uri.file("/workspace/extension"),
    getDefaultCwd: () => "/workspace", ide: {} as never,
    initialize: async () => ({ sessionId: "s1", capabilities: [], slashCommands: [] } as never),
    messenger: { onEvent: () => ({ dispose: eventDispose }), request } as never,
    sessionRouter: {} as never,
  });
  providers.push(provider);
  const host = provider as unknown as {
    bootstrap(): Promise<void>;
    isReady: boolean;
    stateStore: WebviewStateStore;
    postState(): Promise<void>;
  };
  // Bootstrap's RPCs are not under test; the real ready -> state publication is.
  vi.spyOn(host, "bootstrap").mockResolvedValue(undefined);
  let current = makeView();
  provider.resolveWebviewView(current.view);
  const options = { disposeOnRemove: true, throwOnRemove: false, resolveOnFocus: false };
  const commands = vi.spyOn(vscode.commands, "executeCommand").mockImplementation(async (command, ...args) => {
    if (command === "setContext" && args[1] === true) {
      if (options.throwOnRemove) throw new Error("setContext failed");
      if (options.disposeOnRemove) current.dispose();
    }
    if (command === "tomcat.chatView.focus" && options.resolveOnFocus) {
      current = makeView();
      provider.resolveWebviewView(current.view);
      await provider.dispatchTestIntent({ type: "ready", messageId: "replacement-ready" });
    }
    return undefined;
  });
  const warning = vi.spyOn(vscode.window, "showWarningMessage").mockResolvedValue(undefined);
  const error = vi.spyOn(vscode.window, "showErrorMessage").mockResolvedValue(undefined);
  return { provider, host, options, commands, warning, error, eventDispose, request, get current() { return current; } };
}

const advance = (ms: number) => vi.advanceTimersByTimeAsync(ms);
const rebuildCommands = [
  ["setContext", "tomcat.chatView.rebuilding", true],
  ["setContext", "tomcat.chatView.rebuilding", false],
  ["tomcat.chatView.focus", { preserveFocus: true }],
];

describe("webview liveness and recovery", () => {
  it("probes before ready, waits three unanswered pings, then rebuilds without touching the task connection", async () => {
    const f = setup();
    expect(f.host.isReady).toBe(false);
    await advance(15_000);
    expect(f.current.postMessage).toHaveBeenCalledTimes(3);
    expect(f.current.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({ channel: "event", content: { type: "ping" } }));
    expect(f.commands).not.toHaveBeenCalled();
    await advance(5_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
    await advance(60_000);
    expect(f.current.postMessage).toHaveBeenCalledTimes(3);
    expect(f.commands).toHaveBeenCalledTimes(3);
    expect(f.eventDispose).not.toHaveBeenCalled();
    expect(f.request).not.toHaveBeenCalled();
  });

  it("clears missed pings on pong, including repeated long healthy operation", async () => {
    const f = setup();
    for (let cycle = 0; cycle < 8; cycle += 1) {
      await advance(15_000);
      f.current.pong();
    }
    expect(f.current.postMessage).toHaveBeenCalledTimes(24);
    expect(f.commands).not.toHaveBeenCalled();
  });

  it("does not count hidden time and starts the unanswered count afresh when visible", async () => {
    const f = setup();
    await advance(10_000);
    f.current.setVisible(false);
    await advance(300_000);
    expect(f.current.postMessage).toHaveBeenCalledTimes(2);
    expect(f.commands).not.toHaveBeenCalled();
    f.current.setVisible(true);
    await advance(15_000);
    expect(f.commands).not.toHaveBeenCalled();
    await advance(5_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
  });

  it("does not probe in Development mode", async () => {
    const f = setup(vscode.ExtensionMode.Development);
    await advance(120_000);
    expect(f.current.postMessage).not.toHaveBeenCalled();
    expect(f.commands).not.toHaveBeenCalled();
  });

  it.each(["pending", "rejected", "throw"] as const)("does not block detection on a %s workbench hand-off", async kind => {
    const f = setup();
    f.current.postMessage.mockImplementation(() => {
      if (kind === "throw") throw new Error("disposed bridge");
      return kind === "pending" ? new Promise(() => undefined) : Promise.reject(new Error("bridge failure"));
    });
    await advance(20_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
  });

  it.each(["view", "provider"] as const)("stops probing on %s disposal without rebuilding", async owner => {
    const f = setup();
    f.host.isReady = true;
    await advance(5_000);
    if (owner === "view") f.current.dispose(); else f.provider.dispose();
    await advance(60_000);
    expect(f.current.postMessage).toHaveBeenCalledTimes(1);
    expect(f.commands).not.toHaveBeenCalled();
    expect(f.host.isReady).toBe(false);
    await f.host.postState();
    expect(f.current.postMessage).toHaveBeenCalledTimes(1);
  });

  it("ignores a queued pong and dispose from an old view after replacement", async () => {
    const f = setup();
    const old = f.current;
    const replacement = makeView();
    f.provider.resolveWebviewView(replacement.view);
    old.dispose();
    await advance(15_000);
    old.stalePong();
    expect(old.postMessage).not.toHaveBeenCalled();
    expect(replacement.postMessage).toHaveBeenCalledTimes(3);
    // The old pong must not postpone detection of the current view.
    await advance(5_000);
    expect(f.commands).toHaveBeenCalledWith("setContext", "tomcat.chatView.rebuilding", true);
    replacement.dispose();
    await advance(0);
  });

  it("waits for dispose before restoring context and focusing", async () => {
    const f = setup();
    f.options.disposeOnRemove = false;
    await advance(20_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands.slice(0, 1));
    await advance(4_999);
    expect(f.commands).toHaveBeenCalledTimes(1);
    f.current.dispose();
    await advance(0);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
  });

  it.each(["timeout", "command failure"] as const)("resets the context and reports %s rather than leaving the panel hidden", async failure => {
    const f = setup();
    f.options.disposeOnRemove = false;
    f.options.throwOnRemove = failure === "command failure";
    await advance(25_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands.slice(0, 2));
    expect(f.error).toHaveBeenCalledWith(en["host.webviewRebuildFailed"]);
    expect(f.eventDispose).not.toHaveBeenCalled();
  });

  it("reprojects retained history and restarts the timer for the replacement view", async () => {
    const f = setup();
    f.options.resolveOnFocus = true;
    f.host.stateStore.setActiveSession("s1");
    f.host.stateStore.appendMessage("s1", "assistant", "still working");
    await advance(20_000);
    expect(f.host.isReady).toBe(true);
    expect(f.current.postMessage.mock.calls.some(([frame]) => frame.channel === "state" && frame.content.sessionViews.s1.timeline.some(item => item.type === "message" && item.text === "still working"))).toBe(true);
    const before = f.current.postMessage.mock.calls.length;
    await advance(5_000);
    expect(f.current.postMessage).toHaveBeenCalledTimes(before + 1);
    expect(f.eventDispose).not.toHaveBeenCalled();
  });

  it("offers exactly one manual recovery after a repeat failure within 60 seconds", async () => {
    const f = setup();
    f.options.resolveOnFocus = true;
    await advance(40_000);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
    expect(f.warning).toHaveBeenCalledWith(en["host.webviewUnresponsive"], { title: en["host.reloadWebview"] });
    await advance(120_000);
    expect(f.warning).toHaveBeenCalledTimes(1);
    expect(f.commands).toHaveBeenCalledTimes(3);
  });

  it("the manual button uses the same rebuild flow", async () => {
    const f = setup();
    f.options.resolveOnFocus = true;
    f.warning.mockImplementation(async (_message, ...items) => items[0] as never);
    await advance(40_000);
    expect(f.warning).toHaveBeenCalledTimes(1);
    expect(f.commands.mock.calls).toEqual([...rebuildCommands, ...rebuildCommands]);
  });

  it("does not act on a notification answered after provider disposal", async () => {
    const f = setup();
    f.options.resolveOnFocus = true;
    let answer!: () => void;
    f.warning.mockImplementation((_message, ...items) => new Promise(resolve => { answer = () => resolve(items[0] as never); }));
    await advance(40_000);
    f.provider.dispose();
    answer();
    await advance(0);
    expect(f.commands.mock.calls).toEqual(rebuildCommands);
  });

  it("allows automatic recovery again after the 60-second cooldown", async () => {
    const f = setup();
    f.options.resolveOnFocus = true;
    await advance(20_000);
    for (let cycle = 0; cycle < 12; cycle += 1) {
      await advance(5_000);
      f.current.pong();
    }
    await advance(20_000);
    expect(f.warning).not.toHaveBeenCalled();
    expect(f.commands.mock.calls).toEqual([...rebuildCommands, ...rebuildCommands]);
  });
});
