import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { activate, deactivate } from "../src/extension";
import { ComposerDraftStore, DRAFT_WRITE_DEBOUNCE_MS } from "../src/shared/composerDraft";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";

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
    const memento = { get: (_key: string, fallback?: unknown) => fallback, update: async () => undefined };
    await activate({
      extensionPath: "/virtual-extension-for-draft-test",
      extensionUri: vscode.Uri.file("/virtual-extension-for-draft-test"),
      storageUri: vscode.Uri.file("/draft-test/workspace"),
      globalStorageUri: vscode.Uri.file("/draft-test/global"),
      workspaceState: memento, globalState: memento,
      subscriptions,
      extension: { packageJSON: { version: "test" } },
    } as unknown as vscode.ExtensionContext);
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
