import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import { SettingsPanel } from "./SettingsPanel";
import type { InitializeResult } from "../../serveClient/initialize";
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
      expectedCliVersion: "0.1.20",
      extensionUri,
      extensionVersion: "0.1.24",
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
      expectedCliVersion: "0.1.20",
      extensionUri,
      extensionVersion: "0.1.24",
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
    ensureInitialized?: () => Promise<InitializeResult>;
    expectedCliVersion?: string | null;
    extensionVersion?: string | null;
    messenger?: Partial<{
      sendListModels: () => Promise<unknown>;
      sendListConnectors: () => Promise<unknown>;
      sendListProviderKeys: () => Promise<unknown>;
      sendAddConnector: (input: unknown) => Promise<unknown>;
      sendSetProviderKey: (envName: string, value: string) => Promise<unknown>;
      sendRemoveModel: (modelId: string) => Promise<unknown>;
      sendUpsertModel: (model: unknown) => Promise<unknown>;
    }>;
    onModelCatalogChanged?: () => Promise<void> | void;
    clearBuildModelPreference?: (modelId: string) => Promise<boolean | void> | boolean | void;
    selectConnectorWorkspaceRoot?: () => Promise<string | null>;
  }) {
    const messenger = {
      sendListModels: vi.fn().mockResolvedValue({
        payload: { models: [] },
        success: true,
      }),
      sendListConnectors: vi.fn().mockResolvedValue({
        payload: { connectors: [] },
        success: true,
      }),
      sendListProviderKeys: vi.fn().mockResolvedValue({
        payload: { keys: [] },
        success: true,
      }),
      sendAddConnector: vi.fn().mockResolvedValue({ payload: null, success: true }),
      sendSetProviderKey: vi.fn().mockResolvedValue({ payload: null, success: true }),
      sendRemoveModel: vi.fn().mockResolvedValue({ payload: null, success: true }),
      sendUpsertModel: vi.fn().mockResolvedValue({ payload: null, success: true }),
      ...overrides?.messenger,
    };
    const panel = new SettingsPanel({
      ensureInitialized:
        overrides?.ensureInitialized
        ?? (async () => ({
          attachmentRoot: null,
          capabilities: [
            "list_models",
            "list_provider_keys",
            "remove_model",
            "set_provider_key",
            "upsert_model",
          ],
          protocolVersion: 1,
          serverVersion: "0.1.20",
          sessionId: null,
        })),
      expectedCliVersion: overrides?.expectedCliVersion ?? "0.1.20",
      extensionUri: vscode.Uri.file("/tmp/tomcat-ext"),
      extensionVersion: overrides?.extensionVersion ?? "0.1.24",
      messenger: messenger as never,
      clearBuildModelPreference: overrides?.clearBuildModelPreference,
      onModelCatalogChanged: overrides?.onModelCatalogChanged,
      selectConnectorWorkspaceRoot: overrides?.selectConnectorWorkspaceRoot,
    });
    return { messenger, panel };
  }

  it("preserves supportedSpeeds in host state and a subsequent full model edit", async () => {
    const model = { id: "relay/model", modelName: "model", api: "openai-responses", apiKeyEnv: "STUB_KEY", baseUrl: "https://relay.example.test", provider: "relay", source: "user", keyPresent: true, capabilities: { files: false, vision: false, tools: true, reasoning: true, web_search: false }, supportedReasoningLevels: ["high", "max"], supportedSpeeds: ["fast", "ultrafast"], contextWindowOptions: [] };
    const sendUpsertModel = vi.fn().mockResolvedValue({ success: true });
    const { panel } = createPanel({ messenger: { sendListModels: vi.fn().mockResolvedValue({ success: true, payload: { models: [model] } }), sendUpsertModel } });
    try {
      await panel.__testingDispatchIntent({ messageId: "ready-speed", type: "settings.ready" });
      const view = panel.__testingSnapshot().state.models[0];
      expect(view.supportedSpeeds).toEqual(["fast", "ultrafast"]);
      await panel.__testingDispatchIntent({ messageId: "save-speed", type: "upsertModel", data: { model: { ...view, description: "edited only description" } } });
      expect(sendUpsertModel).toHaveBeenCalledWith(expect.objectContaining({ id: "relay/model", description: "edited only description", supportedSpeeds: ["fast", "ultrafast"] }));
    } finally { panel.dispose(); }
  });

  it("publishes route changes before slow model discovery completes", async () => {
    let release!: (value: unknown) => void;
    const models = new Promise((resolve) => { release = resolve; });
    const { panel } = createPanel({ messenger: { sendListModels: () => models } });
    const postMessage = vi.fn().mockResolvedValue(true);
    Object.assign(panel, { panel: { dispose: vi.fn(), webview: { postMessage } } });
    let navigation: Promise<void> | undefined;
    try {
      await panel.__testingDispatchIntent({ type: "settings.ready", messageId: "before-reopen", data: { route: "connectors" } });
      postMessage.mockClear();
      navigation = panel.__testingDispatchIntent({ type: "settings.ready", messageId: "open-models", data: { route: "models" } });
      expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({ channel: "state", content: expect.objectContaining({ route: "models" }) }));
    } finally {
      release({ success: true, payload: { models: [] } });
      await navigation;
      panel.dispose();
    }
  });

  it("retains the serve-provided configuration paths even when no connectors exist", async () => {
    const { messenger, panel } = createPanel({
      ensureInitialized: async () => ({
        attachmentRoot: null,
        capabilities: ["list_connectors"],
        protocolVersion: 1,
        serverVersion: "0.1.20",
        sessionId: null,
      }),
      messenger: {
        sendListConnectors: vi.fn().mockResolvedValue({
          payload: {
            configPaths: {
              global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" },
              workspace: { display: ".workspace-data/mcp.json", raw: "/tmp/project/.workspace-data/mcp.json" },
            },
            connectors: [],
          },
          success: true,
        }),
      },
    });

    await panel.__testingDispatchIntent({
      data: { route: "connectors" },
      messageId: "connector-config-paths",
      type: "settings.ready",
    } satisfies SettingsIntent);

    expect(messenger.sendListConnectors).toHaveBeenCalledTimes(1);
    expect(panel.__testingSnapshot().state.connectorConfigPaths).toEqual({
      global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" },
      workspace: { display: ".workspace-data/mcp.json", raw: "/tmp/project/.workspace-data/mcp.json" },
    });
  });

  it("keeps the global connector config path when the nullable workspace path is absent", async () => {
    const { panel } = createPanel({
      ensureInitialized: async () => ({ attachmentRoot: null, capabilities: ["list_connectors"], protocolVersion: 1, serverVersion: "0.1.20", sessionId: null }),
      messenger: {
        sendListConnectors: vi.fn().mockResolvedValue({
          payload: {
            configPaths: { global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" }, workspace: null },
            connectors: [],
          },
          success: true,
        }),
      },
    });

    await panel.__testingDispatchIntent({ data: { route: "connectors" }, messageId: "global-only-path", type: "settings.ready" } satisfies SettingsIntent);
    expect(panel.__testingSnapshot().state.connectorConfigPaths).toEqual({
      global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" },
    });
  });

  it("uses the explicitly selected workspace for every connector request", async () => {
    const selectConnectorWorkspaceRoot = vi.fn().mockResolvedValue("/workspace-b");
    const { messenger, panel } = createPanel({
      ensureInitialized: async () => ({ attachmentRoot: null, capabilities: ["list_connectors"], protocolVersion: 1, serverVersion: "0.1.20", sessionId: null }),
      selectConnectorWorkspaceRoot,
    });

    await panel.__testingDispatchIntent({
      data: { route: "connectors" },
      messageId: "explicit-workspace-context",
      type: "settings.ready",
    } satisfies SettingsIntent);

    expect(selectConnectorWorkspaceRoot).toHaveBeenCalledTimes(1);
    expect(messenger.sendListConnectors).toHaveBeenCalledWith({ workspaceRoot: "/workspace-b" });
  });

  it("reselects the workspace after the settings view is closed", async () => {
    const selectConnectorWorkspaceRoot = vi
      .fn()
      .mockResolvedValueOnce("/workspace-a")
      .mockResolvedValueOnce("/workspace-b");
    const { messenger, panel } = createPanel({
      ensureInitialized: async () => ({ attachmentRoot: null, capabilities: ["list_connectors"], protocolVersion: 1, serverVersion: "0.1.20", sessionId: null }),
      selectConnectorWorkspaceRoot,
    });

    await panel.__testingDispatchIntent({
      data: { route: "connectors" },
      messageId: "first-workspace",
      type: "settings.ready",
    } satisfies SettingsIntent);
    panel.dispose();
    await panel.__testingDispatchIntent({
      data: { route: "connectors" },
      messageId: "second-workspace",
      type: "settings.ready",
    } satisfies SettingsIntent);

    expect(selectConnectorWorkspaceRoot).toHaveBeenCalledTimes(2);
    expect(messenger.sendListConnectors).toHaveBeenLastCalledWith({ workspaceRoot: "/workspace-b" });
  });

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

    expect(panel.__testingSnapshot().state.extensionVersion).toBe("0.1.24");
    expect(panel.__testingSnapshot().state.expectedCliVersion).toBe("0.1.20");
    expect(panel.__testingSnapshot().state.serverVersion).toBe("0.1.20");
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

  it("keeps a saved connector receipt when post-save setup cannot start a connection", async () => {
    const { panel } = createPanel({
      messenger: {
        sendAddConnector: vi.fn().mockResolvedValue({
          payload: {
            configSaved: true,
            connectionStarted: false,
            postSaveError: "trust store is temporarily unavailable",
          },
          success: true,
        }),
      },
    });

    await panel.__testingDispatchIntent({
      data: {
        connector: {
          auth: "none",
          command: "node",
          name: "saved-partial-connector",
          scope: "global",
          transport: "stdio",
          type: "mcp",
        },
      },
      messageId: "saved-partial-connector",
      type: "addConnector",
    } satisfies SettingsIntent);

    const snapshot = panel.__testingSnapshot().state;
    expect(snapshot.error).toBeNull();
    expect(snapshot.connectorReceipt).toEqual({
      configSaved: true,
      connectionStarted: false,
      error: "trust store is temporarily unavailable",
      name: "saved-partial-connector",
      requestId: "saved-partial-connector",
    });
    expect(snapshot.status).toContain("Connector saved, but connection was not started.");
    expect(snapshot.status).toContain("trust store is temporarily unavailable");
  });

  it("clears the matching Build preference before deletion and forwards warning receipts", async () => {
    const clearBuildModelPreference = vi.fn().mockResolvedValue(undefined);
    const removeModel = vi.fn().mockResolvedValue({
      payload: { modelId: "relay/remove-me", warnings: ["session B was cleared"] },
      success: true,
    });
    const { panel } = createPanel({
      clearBuildModelPreference,
      messenger: { sendRemoveModel: removeModel },
    });

    await panel.__testingDispatchIntent({
      data: { modelId: "relay/remove-me" },
      messageId: "remove-with-warning",
      type: "removeModel",
    } satisfies SettingsIntent);

    expect(clearBuildModelPreference).toHaveBeenCalledWith("relay/remove-me");
    expect(removeModel).toHaveBeenCalledWith("relay/remove-me");
    expect(
      clearBuildModelPreference.mock.invocationCallOrder[0],
    ).toBeLessThan(removeModel.mock.invocationCallOrder[0]);
    expect(panel.__testingSnapshot().state).toMatchObject({
      modelRemovalReceipt: {
        modelId: "relay/remove-me",
        success: true,
        warnings: ["session B was cleared"],
      },
      status: "Model removed with warnings.",
      warnings: ["session B was cleared"],
    });
  });

  it("keeps the model untouched when clearing its Build preference fails", async () => {
    const clearBuildModelPreference = vi.fn().mockRejectedValue(new Error("settings write denied"));
    const { messenger, panel } = createPanel({ clearBuildModelPreference });

    await panel.__testingDispatchIntent({
      data: { modelId: "relay/keep-me" },
      messageId: "remove-build-preference-failure",
      type: "removeModel",
    } satisfies SettingsIntent);

    expect(messenger.sendRemoveModel).not.toHaveBeenCalled();
    expect(panel.__testingSnapshot().state).toMatchObject({
      error: "Error: settings write denied",
      modelRemovalReceipt: { modelId: "relay/keep-me", success: false, warnings: [] },
    });
  });

  it("reports a timed-out delete and lets a later catalog refresh recover", async () => {
    const sendListModels = vi
      .fn()
      .mockResolvedValueOnce({ error: "catalog refresh timed out", success: false })
      .mockResolvedValue({
        payload: { models: [{ id: "relay/survived" }] },
        success: true,
      });
    const { messenger, panel } = createPanel({
      clearBuildModelPreference: vi.fn().mockResolvedValue(true),
      messenger: {
        sendListModels,
        sendRemoveModel: vi.fn().mockRejectedValue(new Error("remove request timed out")),
      },
    });

    await panel.__testingDispatchIntent({
      data: { modelId: "relay/survived" },
      messageId: "remove-timeout",
      type: "removeModel",
    } satisfies SettingsIntent);
    expect(panel.__testingSnapshot().state).toMatchObject({
      error: "Error: remove request timed out",
      modelRemovalReceipt: {
        modelId: "relay/survived",
        success: false,
        warnings: ["The matching Build preference was cleared before deletion."],
      },
      warnings: ["The matching Build preference was cleared before deletion."],
    });
    expect(sendListModels).toHaveBeenCalledTimes(1);

    await panel.__testingDispatchIntent({
      data: { route: "models" },
      messageId: "refresh-after-timeout",
      type: "settings.ready",
    } satisfies SettingsIntent);
    expect(panel.__testingSnapshot().state.models.map((model) => model.id)).toEqual([
      "relay/survived",
    ]);
    expect(panel.__testingSnapshot().state.error).toBeNull();
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

describe("connector Reload ownership and observation", () => {
  type Reply = { success: boolean; payload?: unknown; error?: string };
  const panels: SettingsPanel[] = [];
  function deferred<T = Reply>() {
    let resolve!: (value: T) => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  }
  const connector = (configKey = "key", generation = "1", state = "connected", attempt = 1) => ({
    configKey, name: configKey, source: "global", state, generation, attempt,
    recovery: state === "connecting" ? { phase: "starting", maxAttempts: 3, remainingMs: 100_000 } : null,
    toolCount: 2, toolFilter: { exclude: [] as string[], include: [] as string[] },
  });
  const accepted = (configKey = "key", generation = "2", recoveryTimeoutMs = 111_250): Reply => ({ success: true, payload: { configKey, accepted: true, generation, recoveryTimeoutMs } });
  const catalog = (generation: string, attempt: number, label: string): Reply => ({ success: true, payload: { configKey: "key", generation, attempt, tools: [{ modelName: label, rawName: label, label, description: label, enabled: true }] } });
  const flush = () => vi.advanceTimersByTimeAsync(0);

  function harness(
    selectWorkspace = vi.fn(async (): Promise<string | null> => "/workspace/A"),
    capabilities = ["list_connectors", "add_connector", "trust_project", "reload_connector", "list_connector_tools", "set_connector_tool_enabled", "list_models", "list_provider_keys"],
  ) {
    let rows = [connector()];
    let project = { root: "/workspace/A", trusted: false };
    let exit = () => {};
    const messenger = {
      onExit: vi.fn((listener: () => void) => { exit = listener; return { dispose() {} }; }),
      sendListConnectors: vi.fn(async (): Promise<Reply> => ({ success: true, payload: { connectors: rows, project } })),
      sendTrustProject: vi.fn(async (root: string): Promise<Reply> => ({ success: true, payload: { projectRoot: root, trusted: true } })),
      sendAddConnector: vi.fn(async (_input: unknown): Promise<Reply> => ({ success: true, payload: { configSaved: true, connectionStarted: false } })),
      sendReloadConnector: vi.fn(async (_key: string): Promise<Reply> => accepted()),
      sendListConnectorTools: vi.fn(async (): Promise<Reply> => catalog("1", 1, "initial")),
      sendSetConnectorToolEnabled: vi.fn(async (configKey: string, rawName: string, enabled: boolean): Promise<Reply> => ({
        success: true,
        payload: { configKey, rawName, enabled, configSaved: true, runtimeApplied: true },
      })),
      sendListModels: vi.fn(async (): Promise<Reply> => ({ success: true, payload: { models: [] } })),
      sendListProviderKeys: vi.fn(async (): Promise<Reply> => ({ success: true, payload: { keys: [] } })),
    };
    const panel = new SettingsPanel({
      ensureInitialized: async () => ({ capabilities, protocolVersion: 1, serverVersion: "test", sessionId: null, attachmentRoot: null }),
      expectedCliVersion: null, extensionVersion: null, extensionUri: vscode.Uri.file("/tmp/tomcat-ext"),
      messenger: messenger as never, selectConnectorWorkspaceRoot: selectWorkspace,
    });
    panels.push(panel);
    return {
      panel, messenger, selectWorkspace, exit: () => exit(),
      setRows: (next: typeof rows) => { rows = next; },
      setProject: (trusted: boolean) => { project = { ...project, trusted }; },
      state: () => panel.__testingSnapshot().state,
      ready: (route: "models" | "connectors" = "connectors") => panel.__testingDispatchIntent({ type: "settings.ready", messageId: "ready", data: { route } }),
      trust: (root = "/workspace/A") => panel.__testingDispatchIntent({ type: "trustProject", messageId: "trust-project", data: { projectRoot: root } }),
      add: (scope: "workspace" | "global", trustProject = false) => panel.__testingDispatchIntent({ type: "addConnector", messageId: "add-project", data: {
        connector: { name: "browser", command: "node", transport: "stdio", scope, type: "mcp" },
        ...(trustProject ? { trustProject: true as const } : {}),
      } }),
      reload: (key = "key", messageId = "click") => panel.__testingDispatchIntent({ type: "reloadConnector", messageId, data: { name: key, configKey: key } }),
      refresh: () => panel.__testingDispatchIntent({ type: "listConnectors", messageId: "refresh" }),
      tools: () => panel.__testingDispatchIntent({ type: "listConnectorTools", messageId: "tools", data: { name: "key", configKey: "key" } }),
      toggle: (enabled: boolean) => panel.__testingDispatchIntent({
        type: "setConnectorToolEnabled",
        messageId: `toggle-${enabled}`,
        data: { name: "key", configKey: "key", rawName: "capture", enabled },
      }),
    };
  }
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { panels.splice(0).forEach((panel) => panel.dispose()); vi.clearAllTimers(); vi.useRealTimers(); });

  it("trusts the displayed project once, rejects stale roots, and refreshes the list", async () => {
    const h = harness();
    h.setRows([{ ...connector("browser"), source: "workspace", state: "awaiting_project_trust" }]);
    await h.ready();
    expect(h.state().connectorProject).toEqual({ root: "/workspace/A", trusted: false });
    await h.trust("/workspace/B");
    expect(h.messenger.sendTrustProject).not.toHaveBeenCalled();
    expect(h.state().error).toContain("changed");
    const pending = deferred();
    h.messenger.sendTrustProject.mockReturnValueOnce(pending.promise);
    const first = h.trust();
    await flush();
    const duplicate = h.trust();
    expect(h.state().connectorTrustPending).toBe(true);
    expect(h.messenger.sendTrustProject).toHaveBeenCalledExactlyOnceWith("/workspace/A");
    h.setProject(true);
    pending.resolve({ success: true, payload: { projectRoot: "/workspace/A", trusted: true } });
    await Promise.all([first, duplicate]);
    expect(h.state().connectorProject).toEqual({ root: "/workspace/A", trusted: true });
    expect(h.state().connectorTrustPending).toBe(false);
    expect(h.messenger.sendTrustProject).toHaveBeenCalledTimes(1);
  });

  it("keeps Add and Trust in one mutation and never trusts for Global or an unknown project", async () => {
    const h = harness();
    await h.ready();
    await h.add("workspace", true);
    expect(h.messenger.sendAddConnector).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      scope: "workspace", trustProject: true, context: { workspaceRoot: "/workspace/A" },
    }));
    expect(h.messenger.sendTrustProject).not.toHaveBeenCalled();
    h.setProject(true);
    await h.refresh();
    await h.add("workspace");
    expect(h.messenger.sendAddConnector.mock.lastCall?.[0]).not.toHaveProperty("trustProject");
    await h.add("global");
    expect(h.messenger.sendAddConnector.mock.lastCall?.[0]).not.toHaveProperty("trustProject");
    await h.add("workspace", true);
    expect(h.state().connectorReceipt?.configSaved).toBe(false);
    expect(h.messenger.sendAddConnector).toHaveBeenCalledTimes(3);
  });

  it("refreshes an open Settings list when the first-load prompt trusts its project", async () => {
    const h = harness();
    await h.ready();
    const before = h.messenger.sendListConnectors.mock.calls.length;
    h.setProject(true);
    h.panel.onProjectTrusted();
    await flush();
    expect(h.messenger.sendListConnectors.mock.calls.length).toBe(before + 1);
    expect(h.state().connectorProject?.trusted).toBe(true);
  });

  it("acknowledges a saved single-tool change before its independent refresh", async () => {
    const h = harness();
    await h.ready();

    await h.toggle(false);

    expect(h.messenger.sendSetConnectorToolEnabled).toHaveBeenCalledWith(
      "key",
      "capture",
      false,
      { workspaceRoot: "/workspace/A" },
    );
    expect(h.state().status).toBe("Tool disabled.");
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      requestId: "toggle-false",
      configSaved: true,
      runtimeApplied: true,
    });
    h.messenger.sendSetConnectorToolEnabled.mockResolvedValueOnce({
      success: false,
      error: "cache synchronization failed",
      payload: { configKey: "key", rawName: "capture", enabled: true, configSaved: true, runtimeApplied: false },
    });

    await h.toggle(true);

    expect(h.state().status).toContain("Tool setting saved, but runtime synchronization could not be confirmed.");
    expect(h.messenger.sendSetConnectorToolEnabled).toHaveBeenCalledTimes(2);
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      requestId: "toggle-true",
      configSaved: true,
      runtimeApplied: false,
      error: "cache synchronization failed",
    });
  });

  it("confirms a complete receipt in the current catalog before its refresh returns", async () => {
    const h = harness();
    const initial = { success: true, payload: {
      configKey: "key", generation: "1", attempt: 1,
      tools: [
        { modelName: "capture", rawName: "capture", label: "capture", description: "capture", enabled: true },
        { modelName: "status", rawName: "status", label: "status", description: "status", enabled: true },
      ],
    } };
    const confirmed = { success: true, payload: {
      ...initial.payload,
      tools: [
        { ...initial.payload.tools[0], enabled: false },
        initial.payload.tools[1],
      ],
    } };
    h.messenger.sendListConnectorTools.mockResolvedValueOnce(initial).mockResolvedValue(confirmed);
    await h.ready();
    await h.tools();
    const refresh = deferred();
    h.messenger.sendListConnectors.mockReturnValueOnce(refresh.promise);

    await h.toggle(false);

    expect(h.state().connectorTools).toEqual(confirmed.payload.tools);
    expect(h.state().connectors?.find((entry) => entry.configKey === "key")?.toolCount).toBe(1);
  });

  it("retains the current catalog when a filter-triggered refresh fails", async () => {
    const h = harness();
    const catalogWithCapture = { success: true, payload: {
      configKey: "key", generation: "1", attempt: 1,
      tools: [{ modelName: "capture", rawName: "capture", label: "capture", description: "capture", enabled: true }],
    } };
    h.messenger.sendListConnectorTools
      .mockResolvedValueOnce(catalogWithCapture)
      .mockRejectedValueOnce(new Error("catalog read failed"));
    await h.ready();
    await h.tools();
    h.setRows([{ ...connector(), toolFilter: { exclude: ["capture"], include: [] } }]);

    await h.refresh();
    await flush();

    expect(h.state().connectorTools).toEqual(catalogWithCapture.payload.tools);
    expect(h.state().connectorToolsIdentity).toMatchObject({ configKey: "key", generation: "1", attempt: 1 });
    expect(h.state().error).toContain("catalog read failed");
  });

  it("does not let a pre-write catalog response overwrite a matching receipt", async () => {
    const h = harness();
    const initial = { success: true, payload: {
      configKey: "key", generation: "1", attempt: 1,
      tools: [{ modelName: "capture", rawName: "capture", label: "capture", description: "capture", enabled: true }],
    } };
    const confirmed = { success: true, payload: {
      ...initial.payload,
      tools: [{ ...initial.payload.tools[0], enabled: false }],
    } };
    h.messenger.sendListConnectorTools.mockResolvedValueOnce(initial).mockResolvedValue(confirmed);
    await h.ready();
    await h.tools();
    const old = deferred();
    h.messenger.sendListConnectorTools.mockReturnValueOnce(old.promise);
    const staleRead = h.tools();
    await flush();

    await h.toggle(false);
    old.resolve(initial);
    await staleRead;
    await flush();

    expect(h.state().connectorTools).toEqual(confirmed.payload.tools);
  });

  it("refreshes the management catalog when only the enabled filter changes", async () => {

    const h = harness();
    h.messenger.sendListConnectorTools
      .mockResolvedValueOnce({ success: true, payload: {
        configKey: "key", generation: "1", attempt: 1,
        tools: [{ modelName: "capture", rawName: "capture", label: "capture", description: "capture", enabled: true }],
      } })
      .mockResolvedValueOnce({ success: true, payload: {
        configKey: "key", generation: "1", attempt: 1,
        tools: [{ modelName: "capture", rawName: "capture", label: "capture", description: "capture", enabled: false }],
      } });
    await h.ready();
    await h.tools();
    expect(h.state().connectorTools?.[0].enabled).toBe(true);

    h.setRows([{ ...connector(), toolFilter: { exclude: ["capture"], include: [] } }]);
    await h.refresh();
    await flush();

    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
    expect(h.state().connectorTools?.[0].enabled).toBe(false);
    expect(h.messenger.sendReloadConnector).not.toHaveBeenCalled();
  });

  it("does not send a mutation when the Serve omits the toggle capability", async () => {
    const h = harness(undefined, ["list_connectors", "list_connector_tools", "list_models", "list_provider_keys"]);
    await h.ready();

    await h.toggle(false);

    expect(h.messenger.sendSetConnectorToolEnabled).not.toHaveBeenCalled();
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      configSaved: false,
      runtimeApplied: false,
      error: "This Tomcat Serve does not support changing individual connector tools.",
    });
  });

  it("does not send a mutation for a missing, overridden, or unavailable source", async () => {
    const h = harness();
    await h.ready();
    h.setRows([connector("other")]);
    await h.refresh();

    await h.toggle(false);

    expect(h.messenger.sendSetConnectorToolEnabled).not.toHaveBeenCalled();
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      configSaved: false,
      runtimeApplied: false,
      error: "This connector is not available to change tools.",
    });
  });

  it("does not accept a success receipt for another tool or source", async () => {
    const h = harness();
    await h.ready();
    h.messenger.sendSetConnectorToolEnabled.mockResolvedValueOnce({
      success: true,
      payload: { configKey: "other", rawName: "capture", enabled: false, configSaved: true, runtimeApplied: true },
    });

    await h.toggle(false);

    expect(h.state().status).not.toBe("Tool disabled.");
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      error: expect.stringContaining("protocols do not match"),
    });
  });

  it("publishes terminal receipts when toggle and Reload contend, then disconnects", async () => {
    const h = harness();
    await h.ready();
    const pending = deferred();
    h.messenger.sendSetConnectorToolEnabled.mockReturnValueOnce(pending.promise);
    const toggle = h.toggle(false);
    await flush();

    await h.reload("key", "reload-while-toggle");
    expect(h.messenger.sendReloadConnector).not.toHaveBeenCalled();
    expect(h.state().connectorReloads?.key).toMatchObject({
      requestId: "reload-while-toggle",
      phase: "failed",
      reason: "rejected",
    });

    h.exit();
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      requestId: "toggle-false",
      error: "Connection lost; result unknown.",
    });
    pending.resolve({
      success: true,
      payload: { configKey: "key", rawName: "capture", enabled: false, configSaved: true, runtimeApplied: true },
    });
    await toggle;
  });

  it("rejects a toggle while the same connector is reloading without sending it", async () => {
    const h = harness();
    await h.ready();
    const pending = deferred();
    h.messenger.sendReloadConnector.mockReturnValueOnce(pending.promise);
    const reload = h.reload();
    await flush();

    await h.toggle(false);

    expect(h.messenger.sendSetConnectorToolEnabled).not.toHaveBeenCalled();
    expect(h.state().connectorToolToggles?.[JSON.stringify(["key", "capture"])]).toMatchObject({
      configSaved: false,
      runtimeApplied: false,
      error: "Wait for this connector to finish reloading before changing a tool.",
    });
    pending.resolve(accepted());
    await reload;
  });

  it("publishes busy before awaiting, deduplicates a source, and permits another source", async () => {
    const h = harness(); h.setRows([connector(), connector("other")]); await h.ready();
    const a = deferred(); const b = deferred();
    h.messenger.sendReloadConnector.mockReturnValueOnce(a.promise).mockReturnValueOnce(b.promise);
    const first = h.reload();
    expect(h.state().connectorReloads?.key).toMatchObject({ phase: "pending", requestId: "click" });
    await h.reload("key", "duplicate");
    const second = h.reload("other", "second"); await flush();
    expect(h.messenger.sendReloadConnector).toHaveBeenCalledTimes(2);
    h.setRows([connector("key", "2"), connector("other", "3")]);
    a.resolve(accepted()); b.resolve(accepted("other", "3"));
    await Promise.all([first, second]); await flush();
    expect(h.state().connectorReloads?.key.phase).toBe("succeeded");
    expect(h.state().connectorReloads?.other.phase).toBe("succeeded");
  });

  it("serializes lists and excludes a pre-acknowledgement Connected response", async () => {
    const h = harness(); await h.ready();
    const old = deferred(); h.messenger.sendListConnectors.mockReturnValueOnce(old.promise);
    const reading = h.refresh(); await flush();
    h.setRows([connector("key", "2", "connecting")]);
    await h.reload(); await flush();
    expect(h.state().connectorReloads?.key.phase).toBe("accepted");
    await vi.advanceTimersByTimeAsync(4000);
    expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(2);
    old.resolve({ success: true, payload: { connectors: [connector("key", "2")] } });
    await reading; await flush();
    expect(h.state().connectorReloads?.key.phase).toBe("accepted");
    expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(3);
    h.setRows([connector("key", "2")]); await h.refresh();
    expect(h.state().connectorReloads?.key.phase).toBe("succeeded");
  });

  it("re-reads when the terminal state arrives before the acknowledgement", async () => {
    const h = harness(); await h.ready(); const ack = deferred();
    h.messenger.sendReloadConnector.mockReturnValueOnce(ack.promise);
    const click = h.reload(); await flush();
    h.setRows([connector("key", "2")]); await h.refresh();
    expect(h.state().connectorReloads?.key.phase).toBe("pending");
    const reads = h.messenger.sendListConnectors.mock.calls.length;
    ack.resolve(accepted()); await click; await flush();
    expect(h.messenger.sendListConnectors.mock.calls.length).toBeGreaterThan(reads);
    expect(h.state().connectorReloads?.key.phase).toBe("succeeded");
  });

  it("observes beyond 30 seconds and restores the five-second refresh interval", async () => {
    const h = harness(); await h.ready(); await vi.advanceTimersByTimeAsync(4000);
    expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(1);
    h.setRows([connector("key", "2", "connecting")]); await h.reload(); await flush();
    await vi.advanceTimersByTimeAsync(35_000);
    expect(h.state().connectorReloads?.key.phase).toBe("accepted");
    h.setRows([connector("key", "2")]); await vi.advanceTimersByTimeAsync(1000);
    expect(h.state().connectorReloads?.key.phase).toBe("succeeded");
    const reads = h.messenger.sendListConnectors.mock.calls.length;
    await vi.advanceTimersByTimeAsync(4000); expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(reads);
    await vi.advanceTimersByTimeAsync(1000); expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(reads + 1);
    expect(h.messenger.sendReloadConnector).toHaveBeenCalledTimes(1);
  });

  it("keeps waiting through transient list failures but expires without replay or resurrection", async () => {
    const h = harness(); await h.ready(); h.setRows([connector("key", "2", "connecting")]);
    h.messenger.sendReloadConnector.mockResolvedValueOnce(accepted("key", "2", 1000));
    h.messenger.sendListConnectors.mockRejectedValueOnce(new Error("temporary list failure"));
    await h.reload(); await flush();
    expect(h.state().connectorReloads?.key.phase).toBe("accepted");
    await vi.advanceTimersByTimeAsync(11_000);
    expect(h.state().connectorReloads?.key).toMatchObject({ phase: "unknown", reason: "timeout" });
    h.setRows([connector("key", "2")]); await h.refresh();
    expect(h.state().connectorReloads?.key.phase).toBe("unknown");
    expect(h.messenger.sendReloadConnector).toHaveBeenCalledTimes(1);
  });

  it.each(["rejection", "ack-timeout", "old-protocol"])("settles %s without hanging", async (failure) => {
    const h = harness(); await h.ready();
    if (failure === "rejection") h.messenger.sendReloadConnector.mockResolvedValueOnce({ success: false, error: "blocked" });
    else if (failure === "ack-timeout") h.messenger.sendReloadConnector.mockRejectedValueOnce(new Error("Timed out waiting for response"));
    else h.messenger.sendReloadConnector.mockResolvedValueOnce({ success: true, payload: { reloaded: true } });
    await h.reload(); await flush();
    expect(h.state().connectorReloads?.key.phase).toBe(failure === "rejection" ? "failed" : "unknown");
    if (failure === "old-protocol") expect(h.state().connectorReloads?.key.message).toContain("protocols do not match");
  });

  it("isolates old Serve acknowledgements even when the replacement reuses a generation", async () => {
    const h = harness(); await h.ready(); const ack = deferred();
    h.messenger.sendReloadConnector.mockReturnValueOnce(ack.promise);
    const old = h.reload(); await flush(); h.exit();
    expect(h.state().connectorReloads?.key.phase).toBe("unknown");
    h.setRows([connector("key", "2")]); await h.ready();
    ack.resolve(accepted()); await old; await flush();
    expect(h.state().connectorReloads?.key.phase).toBe("unknown");
    await h.reload("key", "new-serve"); await flush();
    expect(h.state().connectorReloads?.key).toMatchObject({ phase: "succeeded", requestId: "new-serve" });
  });

  it("does not reuse a workspace selection that resolves after closing and reopening", async () => {
    const selection = deferred<string | null>();
    const choose = vi.fn(async (): Promise<string | null> => "/workspace/B").mockReturnValueOnce(selection.promise);
    const h = harness(choose); const first = h.ready(); await flush();
    h.panel.dispose(); const second = h.ready(); await flush();
    selection.resolve("/workspace/A"); await Promise.all([first, second]);
    expect(h.messenger.sendListConnectors).toHaveBeenCalledTimes(1);
    expect(h.messenger.sendListConnectors).toHaveBeenCalledWith({ workspaceRoot: "/workspace/B" });
  });

  it("discards a closed view's receipt for the same key in a different workspace", async () => {
    const choose = vi.fn(async (): Promise<string | null> => "/workspace/B").mockResolvedValueOnce("/workspace/A");
    const h = harness(choose); await h.ready(); const ack = deferred();
    h.messenger.sendReloadConnector.mockReturnValueOnce(ack.promise);
    const old = h.reload(); await flush(); h.panel.dispose();
    await h.ready(); ack.resolve(accepted()); await old; await flush();
    expect(h.state().connectorReloads).toEqual({});
    expect(h.messenger.sendListConnectors).toHaveBeenLastCalledWith({ workspaceRoot: "/workspace/B" });
  });

  it("selects a recovering source without reading tools until the poller observes Connected", async () => {
    const h = harness();
    h.setRows([connector("previous"), connector("key", "2", "connecting", 2)]);
    await h.ready();
    h.messenger.sendListConnectorTools.mockResolvedValueOnce({ success: true, payload: { configKey: "previous", generation: "1", attempt: 1, tools: [] } });
    await h.panel.__testingDispatchIntent({ type: "listConnectorTools", messageId: "old-selection", data: { name: "previous", configKey: "previous" } });
    expect(h.state().selectedConnector).toBe("previous");
    await h.tools();
    expect(h.state().selectedConnector).toBe("key");
    expect(h.state().connectorToolsIdentity).toBeNull();
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(1);
    h.messenger.sendListConnectorTools.mockResolvedValue(catalog("2", 2, "recovered-tools"));
    h.setRows([connector("previous"), connector("key", "2", "connected", 2)]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.state().connectorTools?.[0].label).toBe("recovered-tools");
    expect(h.state().connectorToolsIdentity).toMatchObject({ configKey: "key", generation: "2", attempt: 2 });
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
    expect(h.messenger.sendListConnectorTools).toHaveBeenLastCalledWith("key", { workspaceRoot: "/workspace/A" });
    expect(h.messenger.sendReloadConnector).not.toHaveBeenCalled();
  });

  it("refreshes tools once per actual connection and discards an older attempt's response", async () => {
    const h = harness(); await h.ready(); const old = deferred();
    h.messenger.sendListConnectorTools.mockReturnValueOnce(old.promise).mockResolvedValue(catalog("1", 2, "new-tools"));
    const tools = h.tools(); await flush();
    h.setRows([connector("key", "1", "connected", 2)]); await h.refresh(); await flush();
    expect(h.state().connectorTools?.[0].label).toBe("new-tools");
    old.resolve(catalog("1", 1, "old-tools")); await tools;
    await h.refresh(); await h.refresh();
    expect(h.state().connectorTools?.[0].label).toBe("new-tools");
    expect(h.state().connectorToolsIdentity).toMatchObject({ generation: "1", attempt: 2 });
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
  });

  it.each(["transport-error", "stale-catalog"])("retries a %s directory read without reconnecting or caching failure", async (failure) => {
    const h = harness(); await h.ready();
    if (failure === "transport-error") {
      h.messenger.sendListConnectorTools.mockRejectedValueOnce(new Error("temporary directory failure"));
    } else {
      h.messenger.sendListConnectorTools.mockResolvedValueOnce(catalog("0", 1, "obsolete-tools"));
    }
    await h.tools(); await flush();
    await h.refresh(); await flush();
    expect(h.state().connectorTools?.[0].label).toBe("initial");
    expect(h.state().connectorToolsIdentity).toMatchObject({ generation: "1", attempt: 1 });
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
    expect(h.messenger.sendReloadConnector).not.toHaveBeenCalled();
    await h.refresh(); await flush();
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
  });

  it("bounds repeated stale directory retries to the ordinary polling interval", async () => {
    const h = harness(); await h.ready();
    h.messenger.sendListConnectorTools.mockResolvedValue(catalog("0", 1, "obsolete-tools"));
    await h.tools(); await flush();
    await vi.advanceTimersByTimeAsync(4999);
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.messenger.sendListConnectorTools).toHaveBeenCalledTimes(2);
    expect(h.state().connectorTools).toEqual([]);
    expect(h.messenger.sendReloadConnector).not.toHaveBeenCalled();
  });

  it("does not let a slow model refresh block or overwrite the connector domain", async () => {
    const h = harness(); const models = deferred(); h.messenger.sendListModels.mockReturnValueOnce(models.promise);
    const old = h.ready("models"); await flush(); await h.ready();
    expect(h.state().connectors?.[0].configKey).toBe("key");
    h.setRows([connector("key", "2")]); await h.reload(); await flush();
    models.resolve({ success: true, payload: { models: [] } }); await old;
    expect(h.state().connectorReloads?.key.phase).toBe("succeeded");
    expect(h.state().connectors?.[0].generation).toBe("2");
    expect(h.messenger.sendListModels).toHaveBeenCalledTimes(1);
  });

  it.each(["removed", "superseded", "needs_authorization"])("settles the authoritative %s outcome", async (outcome) => {
    const h = harness(); await h.ready();
    h.setRows([connector("key", "2", "connecting")]); await h.reload(); await flush();
    h.setRows(outcome === "removed" ? [] : [connector("key", outcome === "superseded" ? "3" : "2", outcome === "needs_authorization" ? outcome : "connected")]);
    await h.refresh();
    expect(h.state().connectorReloads?.key).toMatchObject({ phase: "failed", reason: outcome === "needs_authorization" ? "rejected" : outcome });
  });
});
