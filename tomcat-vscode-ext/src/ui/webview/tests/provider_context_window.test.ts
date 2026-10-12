import * as vscode from "vscode";
import { describe, expect, it, vi } from "vitest";

import { TomcatWebviewViewProvider } from "../provider";

describe("context window intent handling", () => {
  it("routes a context tier through serve and refreshes model/session state", async () => {
    const sendSetContextWindow = vi.fn().mockResolvedValue({ success: true });
    const refreshModels = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        sendSetContextWindow,
      } as never,
      sessionRouter: {
        getState: vi.fn().mockResolvedValue({
          busy: false,
          model: "gpt-5.6",
          sessionId: "s1",
          thinkingLevel: "high",
        }),
      } as never,
    });
    vi.spyOn(provider as any, "ensureInitialized").mockResolvedValue(undefined);
    vi.spyOn(provider as any, "ensureWebviewSession").mockResolvedValue("s1");
    vi.spyOn(provider as any, "refreshModels").mockImplementation(refreshModels);
    vi.spyOn(provider as any, "postState").mockResolvedValue(undefined);

    await (
      provider as unknown as {
        handleWebviewMessage(message: unknown): Promise<void>;
      }
    ).handleWebviewMessage({
      data: {
        contextWindow: 1_000_000,
        modelId: "gpt-5.6",
        sessionId: "s1",
      },
      messageId: "context-tier",
      type: "setContextWindow",
    });

    expect(sendSetContextWindow).toHaveBeenCalledWith("s1", "gpt-5.6", 1_000_000);
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(provider.currentState().sessionViews.s1).toMatchObject({
      model: "gpt-5.6",
      sessionId: "s1",
    });
    provider.dispose();
  });
});
