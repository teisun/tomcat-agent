import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import { SettingsPanel } from "./SettingsPanel";
import type { SettingsIntent } from "../../shared/settingsProtocol";

describe("settings panel html asset resolution", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(
      tempDirs.map(async (dir) => {
        await fs.rm(dir, { force: true, recursive: true });
      }),
    );
    tempDirs.length = 0;
  });

  async function createExtensionRoot(files: Record<string, string>): Promise<vscode.Uri> {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-settings-assets-"));
    tempDirs.push(dir);
    await Promise.all(
      Object.entries(files).map(async ([relativePath, contents]) => {
        const filePath = path.join(dir, relativePath);
        await fs.mkdir(path.dirname(filePath), { recursive: true });
        await fs.writeFile(filePath, contents, "utf8");
      }),
    );
    return vscode.Uri.file(dir);
  }

  function createWebview(): vscode.Webview {
    return {
      asWebviewUri(uri: vscode.Uri) {
        return uri;
      },
      cspSource: "vscode-test-webview",
    } as unknown as vscode.Webview;
  }

  it("falls back to another built stylesheet when styles.css is absent", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/settings.js": "console.log('settings');",
      "gui/dist/theme.css": "body { color: blue; }",
    });
    const panel = new SettingsPanel({
      ensureInitialized: async () => ({} as never),
      expectedCliVersion: "0.1.15",
      extensionUri,
      extensionVersion: "0.1.18",
      messenger: {} as never,
    });

    const html = (
      panel as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain('rel="stylesheet"');
    expect(html).toContain("theme.css");
  });

  it("carries every stylesheet the built settings.html declares (codicon.css guard)", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/settings.html": `<!doctype html><html><head>
        <script type="module" crossorigin src="./settings.js"></script>
        <link rel="stylesheet" crossorigin href="./styles.css">
        <link rel="stylesheet" crossorigin href="./codicon.css">
      </head><body><div id="root"></div></body></html>`,
      "gui/dist/settings.js": "console.log('settings');",
      "gui/dist/styles.css": "body { color: blue; }",
      "gui/dist/codicon.css": "@font-face { font-family: codicon; }",
    });
    const panel = new SettingsPanel({
      ensureInitialized: async () => ({} as never),
      expectedCliVersion: "0.1.15",
      extensionUri,
      extensionVersion: "0.1.18",
      messenger: {} as never,
    });

    const html = (
      panel as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain("styles.css");
    expect(html).toContain("codicon.css");
  });
});

describe("settings panel model management flow", () => {
  function createPanel(overrides?: {
    ensureInitialized?: () => Promise<{
      capabilities: string[];
      protocolVersion: number;
      serverVersion: string | null;
      sessionId: string | null;
    }>;
    expectedCliVersion?: string | null;
    extensionVersion?: string | null;
    messenger?: Partial<{
      sendListModels: () => Promise<unknown>;
      sendListProviderKeys: () => Promise<unknown>;
      sendSetProviderKey: (envName: string, value: string) => Promise<unknown>;
      sendUpsertModel: (model: unknown) => Promise<unknown>;
    }>;
    onModelCatalogChanged?: () => Promise<void> | void;
  }) {
    const messenger = {
      sendListModels: vi.fn().mockResolvedValue({
        payload: { models: [] },
        success: true,
      }),
      sendListProviderKeys: vi.fn().mockResolvedValue({
        payload: { keys: [] },
        success: true,
      }),
      sendSetProviderKey: vi.fn().mockResolvedValue({ payload: null, success: true }),
      sendUpsertModel: vi.fn().mockResolvedValue({ payload: null, success: true }),
      ...overrides?.messenger,
    };
    const panel = new SettingsPanel({
      ensureInitialized:
        overrides?.ensureInitialized
        ?? (async () => ({
          capabilities: [
            "list_models",
            "list_provider_keys",
            "remove_model",
            "set_provider_key",
            "upsert_model",
          ],
          protocolVersion: 1,
          serverVersion: "0.1.15",
          sessionId: null,
        })),
      expectedCliVersion: overrides?.expectedCliVersion ?? "0.1.15",
      extensionUri: vscode.Uri.file("/tmp/tomcat-ext"),
      extensionVersion: overrides?.extensionVersion ?? "0.1.18",
      messenger: messenger as never,
      onModelCatalogChanged: overrides?.onModelCatalogChanged,
    });
    return { messenger, panel };
  }

  it("does not persist provider keys when model save fails", async () => {
    const { messenger, panel } = createPanel({
      messenger: {
        sendUpsertModel: vi.fn().mockResolvedValue({
          error: "bad model",
          success: false,
        }),
      },
    });

    await panel.__testingDispatchIntent({
      data: {
        model: {
          api: "openai",
          apiKeyEnv: "OPENAI_API_KEY",
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: true,
            webSearch: false,
          },
          id: "broken-model",
          provider: "openai",
        },
        providerKey: {
          envName: "OPENAI_API_KEY",
          value: "secret",
        },
      },
      messageId: "upsert-with-key",
      type: "upsertModel",
    } satisfies SettingsIntent);

    expect(messenger.sendUpsertModel).toHaveBeenCalledTimes(1);
    expect(messenger.sendSetProviderKey).not.toHaveBeenCalled();
    expect(panel.__testingSnapshot().state.error).toBe("bad model");
  });

  it("surfaces non-fatal model warnings after save", async () => {
    const { panel } = createPanel({
      messenger: {
        sendUpsertModel: vi.fn().mockResolvedValue({
          payload: {
            model: { id: "relay-openai" },
            warnings: [
              "API `openai-responses` expects reasoning effort, but thinking_format=`anthropic` will not send it.",
            ],
          },
          success: true,
        }),
      },
    });

    await panel.__testingDispatchIntent({
      data: {
        model: {
          api: "openai-responses",
          apiKeyEnv: "RELAY_API_KEY",
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: false,
            webSearch: false,
          },
          id: "relay-openai",
          provider: "relay",
          thinkingFormat: "anthropic",
        },
      },
      messageId: "upsert-with-warning",
      type: "upsertModel",
    } satisfies SettingsIntent);

    expect(panel.__testingSnapshot().state.status).toBe("Model saved.");
    expect(panel.__testingSnapshot().state.warnings).toEqual([
      "API `openai-responses` expects reasoning effort, but thinking_format=`anthropic` will not send it.",
    ]);
  });

  it("stores extension and serve version metadata in state snapshots", async () => {
    const { panel } = createPanel();

    await panel.__testingDispatchIntent({
      data: { route: "models" },
      messageId: "version-state",
      type: "settings.ready",
    } satisfies SettingsIntent);

    expect(panel.__testingSnapshot().state.extensionVersion).toBe("0.1.18");
    expect(panel.__testingSnapshot().state.expectedCliVersion).toBe("0.1.15");
    expect(panel.__testingSnapshot().state.serverVersion).toBe("0.1.15");
  });

  it("keeps previous models and exposes list failures", async () => {
    const { messenger, panel } = createPanel({
      messenger: {
        sendListModels: vi
          .fn()
          .mockResolvedValueOnce({
            payload: {
              models: [
                {
                  api: "openai",
                  apiKeyEnv: "OPENAI_API_KEY",
                  capabilities: {
                    files: true,
                    reasoning: true,
                    tools: true,
                    vision: true,
                    web_search: false,
                  },
                  id: "gpt-5.4",
                  keyPresent: true,
                  provider: "openai",
                  source: "builtin",
                },
              ],
            },
            success: true,
          })
          .mockResolvedValueOnce({
            error: "models broken",
            success: false,
          }),
      },
    });

    await panel.__testingDispatchIntent({
      data: { route: "models" },
      messageId: "ready",
      type: "settings.ready",
    } satisfies SettingsIntent);
    expect(panel.__testingSnapshot().state.models.map((model) => model.id)).toEqual([
      "gpt-5.4",
    ]);

    await panel.__testingDispatchIntent({
      messageId: "refresh",
      type: "listModels",
    } satisfies SettingsIntent);

    const snapshot = panel.__testingSnapshot().state;
    expect(snapshot.error).toBe("models broken");
    expect(snapshot.models.map((model) => model.id)).toEqual(["gpt-5.4"]);
  });

  it("refreshes provider keys before models so keyPresent uses the latest env snapshot", async () => {
    const { messenger, panel } = createPanel();

    await panel.__testingDispatchIntent({
      messageId: "refresh-in-order",
      type: "listModels",
    } satisfies SettingsIntent);

    const listProviderKeysMock = vi.mocked(messenger.sendListProviderKeys);
    const listModelsMock = vi.mocked(messenger.sendListModels);
    expect(listProviderKeysMock).toHaveBeenCalledTimes(1);
    expect(listModelsMock).toHaveBeenCalledTimes(1);
    expect(listProviderKeysMock.mock.invocationCallOrder[0]).toBeLessThan(
      listModelsMock.mock.invocationCallOrder[0],
    );
  });

  it("can refresh only provider keys without reloading models", async () => {
    const { messenger, panel } = createPanel();

    await panel.__testingDispatchIntent({
      messageId: "refresh-keys-only",
      type: "listProviderKeys",
    } satisfies SettingsIntent);

    expect(messenger.sendListProviderKeys).toHaveBeenCalledTimes(1);
    expect(messenger.sendListModels).not.toHaveBeenCalled();
  });

  it("marks the webview ready only after the settings.ready handshake arrives", async () => {
    const { panel } = createPanel();

    expect(panel.__testingSnapshot().webviewReady).toBe(false);

    await panel.__testingDispatchIntent({
      data: { route: "models" },
      messageId: "handshake",
      type: "settings.ready",
    } satisfies SettingsIntent);

    expect(panel.__testingSnapshot().webviewReady).toBe(true);
  });
});
