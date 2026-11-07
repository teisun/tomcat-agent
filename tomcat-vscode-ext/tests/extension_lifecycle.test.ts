import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { activate, deactivate } from "../src/extension";
import { ComposerDraftStore, DRAFT_WRITE_DEBOUNCE_MS } from "../src/shared/composerDraft";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import * as installer from "../src/config/installUserCli";
import { TEST_PATH_ENV } from "../src/constants";
import { ServeConnectionSupervisor } from "../src/serveClient/ServeConnectionSupervisor";

// Registration-only Host APIs; lifecycle, provider and storage remain real.
vi.mock("vscode", async importOriginal => {
  const actual = await importOriginal<typeof import("./stubs/vscode")>();
  const registration = () => new actual.Disposable();
  return {
    ...actual,
    languages: { registerCodeLensProvider: registration },
    window: {
      ...actual.window,
      registerWebviewViewProvider: registration,
      registerCustomEditorProvider: registration,
      onDidChangeTextEditorSelection: registration,
      onDidChangeActiveTextEditor: registration,
    },
  };
});

const native = (vscode as typeof vscode & { __testing: typeof import("./stubs/vscode").__testing }).__testing;
const subscriptions: vscode.Disposable[] = [];

afterEach(async () => {
  await deactivate().catch(() => undefined);
  for (const subscription of subscriptions.splice(0)) subscription.dispose();
  vi.useRealTimers();
  vi.unstubAllEnvs();
});

function context(): vscode.ExtensionContext {
  const memento = { get: (_key: string, fallback?: unknown) => fallback, update: async () => undefined };
  return {
    extensionPath: "/virtual-extension-for-draft-test",
    extensionUri: vscode.Uri.file("/virtual-extension-for-draft-test"),
    storageUri: vscode.Uri.file("/draft-test/workspace"),
    globalStorageUri: vscode.Uri.file("/draft-test/global"),
    workspaceState: memento, globalState: memento, subscriptions,
    extension: { packageJSON: { version: "test" } },
  } as unknown as vscode.ExtensionContext;
}

describe("automatic CLI startup boundary", () => {
  it("registers UI and opens logs while installation is pending, but defers connection and resolution", async () => {
    native.reset();
    vi.stubEnv(TEST_PATH_ENV, "/owned-fixture/tomcat");
    let release!: () => void;
    const barrier = new Promise<void>(resolve => { release = resolve; });
    let markStarted!: () => void;
    const started = new Promise<void>(resolve => { markStarted = resolve; });
    vi.spyOn(installer, "installUserCli").mockImplementation(async () => { markStarted(); await barrier; });
    const whenReady = vi.spyOn(ServeConnectionSupervisor.prototype, "whenReady").mockResolvedValue({
      attachmentRoot: null, capabilities: [], protocolVersion: 1, serverVersion: "test", sessionId: null,
    });
    const start = vi.spyOn(TomcatMessenger.prototype, "start").mockImplementation(() => { throw new Error("unexpected CLI startup"); });
    const show = vi.fn();
    vi.spyOn(vscode.window, "createOutputChannel").mockReturnValue({
      appendLine: vi.fn(), show, dispose: vi.fn(),
    } as unknown as vscode.OutputChannel);
    try {
      const api = await activate(context());
      await started;
      let resolved = false;
      const executable = api.__testing.getResolvedExecutable().then(value => { resolved = true; return value; });
      const connecting = api.__testing.sendSettingsIntent({ type: "settings.ready", messageId: "pending-ready", data: { route: "general" } });
      await api.__testing.sendSettingsIntent({ type: "viewConnectionLogs", messageId: "pending-logs" });
      expect(show).toHaveBeenCalledExactlyOnceWith(true);
      expect(resolved).toBe(false);
      expect(whenReady).not.toHaveBeenCalled();
      expect(start).not.toHaveBeenCalled();
      expect(api.__testing.getPromptHistory()).toEqual([]);
      release();
      expect(await executable).toEqual({ executable: "/owned-fixture/tomcat", found: true, source: "test-env" });
      await connecting;
      expect(whenReady).toHaveBeenCalledOnce();
      expect(start).not.toHaveBeenCalled();
    } finally { release(); }
  });
});

describe("extension draft shutdown", () => {
  it.each([
    { inFlight: false, fail: false },
    { inFlight: true, fail: false },
    { inFlight: false, fail: true },
  ])("awaits draft persistence (inFlight=$inFlight, failure=$fail)", async ({ inFlight, fail }) => {
    native.reset();
    vi.useFakeTimers();
    vi.stubEnv("PATH", "");
    // This test must never start a real Serve or access a user's runtime profile.
    const start = vi.spyOn(TomcatMessenger.prototype, "start").mockImplementation(() => {
      throw new Error("unexpected Serve startup in lifecycle test");
    });
    const factory = vi.spyOn(ComposerDraftStore, "forContext");
    await activate(context());
    const store = factory.mock.results[0].value as ComposerDraftStore;
    store.update("shutdown-session", draft => ({ ...draft, text: "last keystroke" }));
    let release!: () => void;
    const barrier = new Promise<void>(resolve => { release = resolve; });
    let markStarted!: () => void;
    const started = new Promise<void>(resolve => { markStarted = resolve; });
    const originalWrite = vscode.workspace.fs.writeFile.bind(vscode.workspace.fs);
    const write = vi.spyOn(vscode.workspace.fs, "writeFile").mockImplementation(async (uri, bytes) => {
      markStarted();
      await barrier;
      if (fail) throw new Error("draft disk unavailable");
      await originalWrite(uri, bytes);
    });
    if (inFlight) await vi.advanceTimersByTimeAsync(DRAFT_WRITE_DEBOUNCE_MS);
    let settled = false;
    let failure: unknown;
    const closing = deactivate().then(() => { settled = true; }, error => { settled = true; failure = error; });
    try {
      // No arbitrary sleep: either the real write starts or shutdown incorrectly returns.
      await Promise.race([started, closing]);
      expect(write).toHaveBeenCalledOnce();
      expect(settled).toBe(false);
      release();
      await closing;
      if (fail) expect(failure).toEqual(expect.objectContaining({ message: "draft disk unavailable" }));
      else {
        expect(failure).toBeUndefined();
        const restored = ComposerDraftStore.forContext({ storageUri: vscode.Uri.file("/draft-test/workspace"), globalStorageUri: vscode.Uri.file("/draft-test/global") });
        expect((await restored.hydrate("shutdown-session")).text).toBe("last keystroke");
      }
      expect(start).not.toHaveBeenCalled();
    } finally {
      release();
      await closing;
    }
  });
});
