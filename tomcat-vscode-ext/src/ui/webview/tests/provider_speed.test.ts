import * as vscode from "vscode";
import { describe, expect, it, vi } from "vitest";
import { parseModelCatalog, TomcatWebviewViewProvider } from "../provider";

type TestBridge = {
  ensureInitialized(): Promise<unknown>;
  ensureWebviewSession(): Promise<string>;
  refreshModels(): Promise<void>;
  refreshSessionState(): Promise<void>;
  postState(): Promise<void>;
  handleWebviewMessage(message: unknown): Promise<void>;
  stateStore: { appendMessage(sessionId: string, kind: string, text: string): unknown };
};

describe("speed intent handling", () => {
  it.each(["success", "rejected", "transport"])("handles %s with the effort refresh/error pattern", async (outcome) => {
    const sendSetSpeed = outcome === "transport"
      ? vi.fn().mockRejectedValue(new Error("offline"))
      : vi.fn().mockResolvedValue({ success: outcome === "success", error: "invalid_speed" });
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"), getDefaultCwd: () => "/workspace",
      ide: {} as never, initialize: async () => ({} as never), sessionRouter: {} as never,
      messenger: { onEvent: () => ({ dispose() {} }), sendSetSpeed } as never,
    });
    const bridge = provider as unknown as TestBridge;
    vi.spyOn(bridge, "ensureInitialized").mockResolvedValue(undefined);
    vi.spyOn(bridge, "ensureWebviewSession").mockResolvedValue("s1");
    const refresh = vi.spyOn(bridge, "refreshModels").mockResolvedValue(undefined);
    const refreshSession = vi.spyOn(bridge, "refreshSessionState").mockResolvedValue(undefined);
    const post = vi.spyOn(bridge, "postState").mockResolvedValue(undefined);
    const error = vi.spyOn(bridge.stateStore, "appendMessage");
    await bridge.handleWebviewMessage({ messageId: "speed", type: "setSpeed", data: { modelId: "relay/model", speed: "ultrafast", sessionId: "s1" } });
    expect(sendSetSpeed).toHaveBeenCalledWith("s1", "relay/model", "ultrafast");
    expect(refresh).toHaveBeenCalledTimes(outcome === "success" ? 1 : 0);
    expect(refreshSession).toHaveBeenCalledWith("s1", { trustBusy: true });
    expect(post).toHaveBeenCalled();
    if (outcome === "success") expect(error).not.toHaveBeenCalled();
    else expect(error).toHaveBeenCalledWith("s1", "error", expect.stringContaining(outcome === "rejected" ? "invalid_speed" : "offline"));
    provider.dispose();
  });
  it("keeps declared and selected speeds in the catalog whitelist", () => {
    const catalog = parseModelCatalog({ models: [
      { id: "relay", keyPresent: true, supportedSpeeds: ["fast", "ultrafast", "warp", 1], selectedSpeed: "fast" },
      { id: "old", keyPresent: true },
    ] });
    expect(catalog.modelDetails.relay).toMatchObject({ supportedSpeeds: ["fast", "ultrafast"], selectedSpeed: "fast" });
    expect(catalog.modelDetails.old).toMatchObject({ supportedSpeeds: [], selectedSpeed: null });
  });
});
