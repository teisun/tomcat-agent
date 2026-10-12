import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { SettingsPanel } from "./SettingsPanel";
import { getLocale, setLocale, subscribeLocale } from "../../shared/i18n";
import type { InitializeResult } from "../../serveClient/initialize";

const init = (): InitializeResult => ({ attachmentRoot: null, capabilities: ["set_ui_language"], protocolVersion: 2, sessionId: null, serverVersion: "test", uiPreferences: { language: "auto", effective: "en", envOverride: false } });
function create(ensureInitialized = async () => init()) {
  const request = vi.fn().mockResolvedValue({ success: true, payload: { language: "zh-CN", effective: "zh-CN", envOverride: false } });
  const panel = new SettingsPanel({ ensureInitialized, expectedCliVersion: null, extensionVersion: null, extensionUri: vscode.Uri.file("/tmp/locale-test"), messenger: { request } as never });
  return { panel, request };
}
afterEach(() => setLocale("en"));

describe("Settings language persistence", () => {
  it("loads General independently of model capability and broadcasts only acknowledged changes", async () => {
    const { panel, request } = create();
    const changed = vi.fn();
    const unsubscribe = subscribeLocale(changed);
    try {
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "ready", data: { route: "general" } });
      expect(panel.__testingSnapshot().state.uiPreferences?.language).toBe("auto");
      await panel.__testingDispatchIntent({ type: "setUiLanguage", messageId: "set", data: { language: "zh-CN" } });
      expect(request).toHaveBeenCalledWith({ type: "set_ui_language", language: "zh-CN" });
      expect(getLocale()).toBe("zh-CN");
      expect(changed).toHaveBeenCalledTimes(1);
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "return", data: { route: "general" } });
      expect(panel.__testingSnapshot().state.uiPreferences?.language).toBe("zh-CN");
      request.mockResolvedValueOnce({ success: false, error: "read-only disk" });
      await panel.__testingDispatchIntent({ type: "setUiLanguage", messageId: "failed", data: { language: "en" } });
      expect(getLocale()).toBe("zh-CN");
      expect(changed).toHaveBeenCalledTimes(1);
      expect(panel.__testingSnapshot().state.languageStatus).toBe("failed");
    } finally { unsubscribe(); panel.dispose(); }
  });
  it("adopts a new ready snapshot without onExit, even when only preference metadata changes", async () => {
    let initialized = init();
    const { panel, request } = create(async () => initialized);
    try {
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "ready", data: { route: "general" } });
      await panel.__testingDispatchIntent({ type: "setUiLanguage", messageId: "saved", data: { language: "zh-CN" } });
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "models", data: { route: "models" } });
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "return", data: { route: "general" } });
      expect(panel.__testingSnapshot().state.uiPreferences?.language).toBe("zh-CN");
      initialized = { ...init(), uiPreferences: { language: "en", effective: "en", envOverride: false } };
      await panel.onServeConnectionChanged(true);
      expect(panel.__testingSnapshot().state.uiPreferences).toEqual(initialized.uiPreferences);
      expect(panel.__testingSnapshot().state.languageStatus).toBeUndefined();
      initialized = { ...init(), uiPreferences: { language: "auto", effective: "en", envOverride: true } };
      await panel.onServeConnectionChanged(true);
      expect(panel.__testingSnapshot().state.uiPreferences).toEqual(initialized.uiPreferences);
      const pendingRefresh = panel.__testingDispatchIntent({ type: "settings.ready", messageId: "refresh-before-restart", data: { route: "general" } });
      await panel.onServeConnectionChanged(false);
      await pendingRefresh;
      expect(panel.__testingSnapshot().state.ready).toBe(false);
      const saves = request.mock.calls.length;
      await panel.__testingDispatchIntent({ type: "setUiLanguage", messageId: "reconnecting", data: { language: "en" } });
      expect(request).toHaveBeenCalledTimes(saves);
    } finally { panel.dispose(); }
  });
  it("keeps General available on a disconnected or old backend without claiming a save", async () => {
    const { panel, request } = create(async () => { throw new Error("offline"); });
    try {
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "ready", data: { route: "general" } });
      expect(panel.__testingSnapshot().state.ready).toBe(false);
      expect(panel.__testingSnapshot().state.error).toContain("offline");
      await panel.__testingDispatchIntent({ type: "setUiLanguage", messageId: "set", data: { language: "en" } });
      expect(request).not.toHaveBeenCalled();
    } finally { panel.dispose(); }
  });
});
