import fs from "node:fs";
import * as os from "node:os";
import path from "node:path";

import * as vscode from "vscode";

import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import {
  hasServeCapability,
  type InitializeResult,
  SERVE_CAPABILITY_LIST_MODELS,
  SERVE_CAPABILITY_LIST_PROVIDER_KEYS,
  SERVE_CAPABILITY_REMOVE_MODEL,
  SERVE_CAPABILITY_SET_PROVIDER_KEY,
  SERVE_CAPABILITY_UPSERT_MODEL,
} from "../../serveClient/initialize";
import type {
  ListModelsPayload,
  ListProviderKeysPayload,
  ModelEntryInput,
  ModelView as WireModelView,
  ProviderKeyView as WireProviderKeyView,
} from "../../serveClient/wire";
import type {
  SettingsCapabilities,
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsProviderKeyInput,
  SettingsProviderKeyView,
  SettingsRoute,
  SettingsStateSnapshot,
} from "../../shared/settingsProtocol";
import { isSettingsIntent as isSettingsIntentMessage } from "../../shared/settingsProtocol";
import { resolveWebviewEntryAssets } from "../guiAssets";
import type {
  ConnectorConfigPath,
  ConnectorConfigPaths,
  ConnectorInput,
  ConnectorToolFilter,
  ConnectorToolView,
  ConnectorView,
} from "../../shared/connectorsProtocol";
import { normalizeConnectorView } from "../../shared/connectorsProtocol";

const CONNECTOR_CAPABILITIES = {
  add: "add_connector",
  filter: "set_connector_tool_filter",
  list: "list_connectors",
  listTools: "list_connector_tools",
  login: "login_connector",
  reload: "reload_connector",
  remove: "remove_connector",
  trust: "set_connector_trust",
} as const;

function getNonce(): string {
  return (
    Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseCapabilities(value: unknown): SettingsModelCapabilities {
  if (!isRecord(value)) {
    return {
      files: false,
      reasoning: false,
      tools: false,
      vision: false,
      webSearch: false,
    };
  }
  return {
    files: value.files === true,
    reasoning: value.reasoning === true,
    tools: value.tools === true,
    vision: value.vision === true,
    webSearch: value.webSearch === true || value.web_search === true,
  };
}

function parseStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string")
    : [];
}

function parseNumberArray(value: unknown): number[] {
  return Array.isArray(value)
    ? value.filter(
        (entry): entry is number =>
          typeof entry === "number" && Number.isInteger(entry) && entry > 0,
      )
    : [];
}

function parseModelView(value: WireModelView): SettingsModelView {
  return {
    api: value.api,
    apiKeyEnv: value.apiKeyEnv,
    baseUrl: value.baseUrl ?? null,
    capabilities: parseCapabilities(value.capabilities),
    contextWindow:
      typeof value.contextWindow === "number" ? value.contextWindow : null,
    contextWindowOptions: parseNumberArray(value.contextWindowOptions),
    description: value.description ?? null,
    id: value.id,
    keyPresent: value.keyPresent === true,
    maxOutputTokens:
      typeof value.maxOutputTokens === "number" ? value.maxOutputTokens : null,
    modelName: value.modelName ?? null,
    provider: value.provider,
    source: value.source === "user" ? "user" : "builtin",
    supportedReasoningLevels: parseStringArray(value.supportedReasoningLevels),
    thinkingFormat: value.thinkingFormat ?? null,
  };
}

function parseProviderKeyView(
  value: WireProviderKeyView,
): SettingsProviderKeyView {
  return {
    envName: value.envName,
    keyPresent: value.keyPresent === true,
    modelIds: value.modelIds,
    provider: value.provider,
  };
}

function parseModelsPayload(
  payload: ListModelsPayload | undefined,
): SettingsModelView[] {
  return payload?.models?.map(parseModelView) ?? [];
}

function parseProviderKeysPayload(
  payload: ListProviderKeysPayload | undefined,
): SettingsProviderKeyView[] {
  return payload?.keys?.map(parseProviderKeyView) ?? [];
}

function parseConnectorConfigPath(value: unknown): ConnectorConfigPath | undefined {
  if (!isRecord(value) || typeof value.display !== "string" || typeof value.raw !== "string") {
    return undefined;
  }
  return { display: value.display, raw: value.raw };
}

function parseConnectorConfigPaths(payload: unknown): ConnectorConfigPaths | undefined {
  if (!isRecord(payload) || !isRecord(payload.configPaths)) return undefined;
  const global = parseConnectorConfigPath(payload.configPaths.global);
  const workspace = parseConnectorConfigPath(payload.configPaths.workspace);
  return global && workspace ? { global, workspace } : undefined;
}

function parseConnectorsPayload(payload: unknown): {
  connectors: ConnectorView[];
  configPaths?: ConnectorConfigPaths;
} {
  if (!isRecord(payload) || !Array.isArray(payload.connectors)) return { connectors: [] };
  return {
    connectors: payload.connectors
      .map(normalizeConnectorView)
      .filter((connector): connector is ConnectorView => connector !== null),
    configPaths: parseConnectorConfigPaths(payload),
  };
}

function parseConnectorToolsPayload(payload: unknown): ConnectorToolView[] {
  if (!isRecord(payload) || !Array.isArray(payload.tools)) return [];
  return payload.tools.flatMap((value) => {
    if (!isRecord(value)) return [];
    const modelName = typeof value.modelName === "string"
      ? value.modelName
      : typeof value.name === "string"
        ? value.name
        : null;
    if (!modelName) return [];
    return [{
      modelName,
      rawName: typeof value.rawName === "string"
        ? value.rawName
        : typeof value.raw_name === "string"
          ? value.raw_name
          : modelName,
      label: typeof value.label === "string" ? value.label : modelName,
      description: typeof value.description === "string" ? value.description : "",
      inputSchema: value.inputSchema,
      enabled: value.enabled !== false,
    }];
  });
}

function toWireModelEntryInput(model: SettingsModelInput): ModelEntryInput {
  return {
    api: model.api,
    apiKeyEnv: model.apiKeyEnv ?? null,
    baseUrl: model.baseUrl ?? null,
    capabilities: {
      files: model.capabilities.files,
      reasoning: model.capabilities.reasoning,
      tools: model.capabilities.tools,
      vision: model.capabilities.vision,
      web_search: model.capabilities.webSearch,
    },
    contextWindow: model.contextWindow ?? null,
    contextWindowOptions: model.contextWindowOptions ?? null,
    description: model.description ?? null,
    id: model.id,
    maxOutputTokens: model.maxOutputTokens ?? null,
    modelName: model.modelName ?? null,
    provider: model.provider,
    supportedReasoningLevels: model.supportedReasoningLevels ?? null,
    thinkingFormat: model.thinkingFormat ?? null,
  };
}

export interface SettingsPanelDeps {
  ensureInitialized(): Promise<InitializeResult>;
  expectedCliVersion: string | null;
  extensionUri: vscode.Uri;
  extensionVersion: string | null;
  messenger: TomcatMessenger;
  onModelCatalogChanged?(): Promise<void> | void;
}

type SettingsDomAction = {
  kind: "clickTestId" | "setInputValue";
  testId?: string;
  value?: string;
};

export type SettingsDomRect = {
  height: number;
  left: number;
  top: number;
  width: number;
};

export type SettingsDomSnapshot = {
  html: string;
  rects?: {
    apiKeyInput?: SettingsDomRect;
    keySlotBox?: SettingsDomRect;
    keySlotInput?: SettingsDomRect;
  };
};

function parseSettingsDomRect(value: unknown): SettingsDomRect | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { height, left, top, width } = value;
  if (
    typeof height === "number" &&
    typeof left === "number" &&
    typeof top === "number" &&
    typeof width === "number"
  ) {
    return { height, left, top, width };
  }
  return undefined;
}

function parseSettingsDomRects(value: unknown): SettingsDomSnapshot["rects"] {
  if (!isRecord(value)) {
    return undefined;
  }
  const apiKeyInput = parseSettingsDomRect(value.apiKeyInput);
  const keySlotBox = parseSettingsDomRect(value.keySlotBox);
  const keySlotInput = parseSettingsDomRect(value.keySlotInput);
  if (!apiKeyInput && !keySlotBox && !keySlotInput) {
    return undefined;
  }
  return { apiKeyInput, keySlotBox, keySlotInput };
}

export class SettingsPanel implements vscode.Disposable {
  private panel?: vscode.WebviewPanel;
  private webviewReady = false;
  private readonly pendingDomSnapshots = new Map<
    string,
    {
      reject(error: Error): void;
      resolve(snapshot: SettingsDomSnapshot): void;
      timeout: ReturnType<typeof setTimeout>;
    }
  >();
  private route: SettingsRoute = "models";
  private connectorRefreshTimer?: ReturnType<typeof setInterval>;
  private state: SettingsStateSnapshot = {
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,
    },
    expectedCliVersion: null,
    extensionVersion: null,
    models: [],
    providerKeys: [],
    connectors: [],
    connectorTools: [],
    selectedConnector: null,
    ready: false,
    route: "models",
    serverVersion: null,
    warnings: null,
  };

  constructor(private readonly deps: SettingsPanelDeps) {}

  private shouldPreserveFocus(): boolean {
    return process.env.TOMCAT_E2E_SCREENSHOT !== "1";
  }

  dispose(): void {
    if (this.connectorRefreshTimer) {
      clearInterval(this.connectorRefreshTimer);
      this.connectorRefreshTimer = undefined;
    }
    for (const pending of this.pendingDomSnapshots.values()) {
      clearTimeout(pending.timeout);
      pending.reject(
        new Error("Settings panel disposed before DOM snapshot completed."),
      );
    }
    this.pendingDomSnapshots.clear();
    this.panel?.dispose();
    this.panel = undefined;
  }

  reveal(route: SettingsRoute = "models"): void {
    this.route = route;
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active, this.shouldPreserveFocus());
      void this.refreshState();
      return;
    }
    this.webviewReady = false;
    this.panel = vscode.window.createWebviewPanel(
      "tomcat.settings",
      "Tomcat Settings",
      {
        preserveFocus: this.shouldPreserveFocus(),
        viewColumn: vscode.ViewColumn.Active,
      },
      {
        enableScripts: true,
        localResourceRoots: [
          vscode.Uri.joinPath(this.deps.extensionUri, "gui", "dist"),
        ],
        retainContextWhenHidden: true,
      },
    );
    this.panel.onDidDispose(() => {
      for (const pending of this.pendingDomSnapshots.values()) {
        clearTimeout(pending.timeout);
        pending.reject(
          new Error("Settings panel closed before DOM snapshot completed."),
        );
      }
      this.pendingDomSnapshots.clear();
      this.webviewReady = false;
      this.panel = undefined;
    });
    this.panel.webview.onDidReceiveMessage((message: unknown) => {
      if (
        isRecord(message) &&
        message.type === "__test.dom_snapshot" &&
        typeof message.messageId === "string"
      ) {
        const pending = this.pendingDomSnapshots.get(message.messageId);
        if (!pending) {
          return;
        }
        clearTimeout(pending.timeout);
        this.pendingDomSnapshots.delete(message.messageId);
        const rawData = isRecord(message.data) ? message.data : {};
        const html = typeof rawData.html === "string" ? rawData.html : "";
        const rects = parseSettingsDomRects(rawData.rects);
        pending.resolve(rects ? { html, rects } : { html });
        return;
      }
      if (!isSettingsIntentMessage(message)) {
        return;
      }
      void this.handleIntent(message);
    });
    this.panel.webview.html = this.renderHtml(this.panel.webview);
    void this.refreshState();
  }

  __testingSnapshot(): {
    route: SettingsRoute;
    state: SettingsStateSnapshot;
    visible: boolean;
    webviewReady: boolean;
  } {
    return {
      route: this.route,
      state: JSON.parse(JSON.stringify(this.state)) as SettingsStateSnapshot,
      visible: Boolean(this.panel?.visible),
      webviewReady: this.webviewReady,
    };
  }

  async __testingDispatchIntent(intent: SettingsIntent): Promise<void> {
    await this.handleIntent(intent);
  }

  async __testingCaptureDom(): Promise<SettingsDomSnapshot> {
    if (!this.panel) {
      throw new Error("Settings panel is not open.");
    }
    const messageId = `settings-dom-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    return new Promise<SettingsDomSnapshot>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pendingDomSnapshots.delete(messageId);
        reject(new Error("Timed out waiting for settings DOM snapshot."));
      }, 20_000);
      this.pendingDomSnapshots.set(messageId, { reject, resolve, timeout });
      void this.panel?.webview.postMessage({
        channel: "event",
        content: {
          type: "__test.capture_dom",
        },
        messageId,
      });
    });
  }

  async __testingDispatchDomAction(action: SettingsDomAction): Promise<void> {
    if (!this.panel) {
      throw new Error("Settings panel is not open.");
    }
    await this.panel.webview.postMessage({
      channel: "event",
      content: {
        action,
        type: "__test.dom_action",
      },
      messageId: `settings-dom-action-${Date.now()}`,
    });
  }

  private async handleIntent(intent: SettingsIntent): Promise<void> {
    switch (intent.type) {
      case "settings.ready":
        this.webviewReady = true;
        this.route = intent.data?.route ?? this.route;
        if (this.route === "connectors" && !this.connectorRefreshTimer) {
          this.connectorRefreshTimer = setInterval(() => {
            void this.refreshState();
          }, 5000);
        } else if (this.route !== "connectors" && this.connectorRefreshTimer) {
          clearInterval(this.connectorRefreshTimer);
          this.connectorRefreshTimer = undefined;
        }
        await this.refreshState();
        return;
      case "listModels":
        await this.refreshState();
        return;
      case "listProviderKeys":
        await this.refreshProviderKeys();
        return;
      case "upsertModel":
        await this.handleUpsertModel(
          intent.data.model,
          intent.data.providerKey,
        );
        return;
      case "removeModel":
        await this.handleRemoveModel(intent.data.modelId);
        return;
      case "setProviderKey":
        await this.handleSetProviderKey(intent.data.envName, intent.data.value);
        return;
      case "listConnectors":
      case "reloadConnectors":
        await this.refreshState();
        return;
      case "listConnectorTools": {
        const response = await this.deps.messenger.sendListConnectorTools(intent.data.name);
        this.state = {
          ...this.state,
          connectorTools: response.success ? parseConnectorToolsPayload(response.payload) : [],
          error: response.success ? null : response.error ?? "Unable to load connector tools.",
          selectedConnector: intent.data.name,
        };
        this.postState();
        return;
      }
      case "addConnector":
        await this.handleAddConnector(intent.data.connector);
        return;
      case "removeConnector":
      case "reloadConnector":
      case "loginConnector":
      case "cancelLoginConnector":
      case "logoutConnector":
      case "trustConnector":
      case "denyConnector":
        await this.handleConnectorAction(intent.type, intent.data.name);
        return;
      case "openConnectorConfig":
        await this.openConnectorConfig(intent.data.name, intent.data.scope);
        return;
      case "setConnectorToolFilter": {
        const connectorScope = this.state.connectors?.find((connector) => connector.name === intent.data.name)?.source === "Global"
          ? "user"
          : "workspace";
        const response = await this.deps.messenger.sendSetConnectorToolFilter(
          intent.data.name,
          intent.data.filter.include,
          intent.data.filter.exclude,
          connectorScope,
        );
        await this.refreshState(
          response.success ? null : response.error ?? "Unable to update connector tools.",
          response.success ? "Connector tools updated." : null,
        );
        return;
      }
    }
  }

  private async openConnectorConfig(
    name?: string,
    scope?: "user" | "workspace",
  ): Promise<void> {
    const connector = name
      ? this.state.connectors?.find((entry) => entry.name === name)
      : undefined;
    const scopedConfigPath = scope === "user"
      ? this.state.connectorConfigPaths?.global
      : scope === "workspace"
        ? this.state.connectorConfigPaths?.workspace
        : undefined;
    const rawConfigPath = connector?.configPathRaw ?? scopedConfigPath?.raw;
    const configPath =
      rawConfigPath && rawConfigPath.startsWith("~")
        ? path.join(os.homedir(), rawConfigPath.slice(2))
        : rawConfigPath;
    if (!configPath) {
      await this.refreshState("The connector configuration file is not available to open.");
      return;
    }
    try {
      if (!fs.existsSync(configPath)) {
        fs.mkdirSync(path.dirname(configPath), { recursive: true });
        try {
          fs.writeFileSync(
            configPath,
            '{\n  "mcpServers": {}\n}\n',
            { encoding: "utf8", flag: "wx" },
          );
        } catch (error) {
          if (!(isRecord(error) && error.code === "EEXIST")) {
            throw error;
          }
        }
      }
      const document = await vscode.workspace.openTextDocument(vscode.Uri.file(configPath));
      await vscode.window.showTextDocument(document, { preview: false });
    } catch (error) {
      await this.refreshState(`Unable to open connector configuration: ${String(error)}`);
    }
  }

  private async handleUpsertModel(

    model: SettingsModelInput,
    providerKey?: SettingsProviderKeyInput,
  ): Promise<void> {
    try {
      const capabilities = this.buildCapabilities(
        await this.deps.ensureInitialized(),
      );
      if (!capabilities.upsertModel) {
        await this.refreshState(
          "Model management is unavailable for this serve instance.",
        );
        return;
      }
      const response = await this.deps.messenger.sendUpsertModel(
        toWireModelEntryInput(model),
      );
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to save model.");
        return;
      }
      const warnings = response.payload?.warnings ?? null;
      if (providerKey) {
        if (!capabilities.setProviderKey) {
          await this.refreshState(
            "Model saved, but this serve instance cannot store API keys yet.",
            null,
            warnings,
          );
          await this.deps.onModelCatalogChanged?.();
          return;
        }
        const keyResponse = await this.deps.messenger.sendSetProviderKey(
          providerKey.envName,
          providerKey.value,
        );
        if (!keyResponse.success) {
          await this.refreshState(
            `Model saved, but API key was not stored: ${keyResponse.error ?? "Unknown error."}`,
            null,
            warnings,
          );
          await this.deps.onModelCatalogChanged?.();
          return;
        }
        await this.refreshState(
          null,
          `Saved ${providerKey.envName}.`,
          warnings,
        );
        await this.deps.onModelCatalogChanged?.();
        return;
      }
      await this.refreshState(null, "Model saved.", warnings);
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async handleAddConnector(input: ConnectorInput): Promise<void> {
    try {
      const response = await this.deps.messenger.sendAddConnector({
        args: input.args ?? [],
        command: input.command ?? "",
        auth: input.auth,
        env: input.env,
        headers: input.headers,
        name: input.name,
        oauth: input.oauth,
        scope: input.scope,
        url: input.url,
      });
      if (!response.success) {
        await this.refreshState(
          response.error ?? "Unable to add connector.",
          "Connector add failed.",
        );
        return;      }
      if (input.transport === "http" && input.oauth !== undefined) {
        await this.refreshState(null, "Authorizing connector…");
        const login = await this.deps.messenger.sendLoginConnector(input.name);
        if (login.success && (login.payload as { authorizing?: boolean } | undefined)?.authorizing) {
          return;
        }
        await this.refreshState(
          login.success ? null : login.error ?? "Connector saved, but OAuth authorization failed.",
          login.success ? "Connector authorized and connected." : "Connector saved; authorization is still required.",
        );
        return;      }
      await this.refreshState(null, "Connector saved.");
    } catch (error) {
      await this.refreshState(String(error), "Connector add failed.");    }
  }

  private async handleConnectorAction(
    action: "removeConnector" | "reloadConnector" | "loginConnector" | "logoutConnector" | "trustConnector" | "denyConnector" | "cancelLoginConnector",
    name: string,
  ): Promise<void> {
    const response = action === "removeConnector"
      ? await this.deps.messenger.sendRemoveConnector(name)
      : action === "trustConnector" || action === "denyConnector"
        ? await this.deps.messenger.sendSetConnectorTrust(name, action === "trustConnector")
        : action === "reloadConnector"
          ? await this.deps.messenger.sendReloadConnector()
          : action === "loginConnector"
            ? await this.deps.messenger.sendLoginConnector(name)
            : action === "cancelLoginConnector"
              ? await this.deps.messenger.sendCancelLoginConnector(name)
              : await this.deps.messenger.sendLogoutConnector(name);
    await this.refreshState(      response.success ? null : response.error ?? "Connector operation failed.",
      response.success
        ? action === "loginConnector"
          ? "Authorizing connector…"
          : "Connector updated."
        : null,
    );
  }

  private async handleRemoveModel(modelId: string): Promise<void> {    try {
      const capabilities = this.buildCapabilities(
        await this.deps.ensureInitialized(),
      );
      if (!capabilities.removeModel) {
        await this.refreshState(
          "Model removal is unavailable for this serve instance.",
        );
        return;
      }
      const response = await this.deps.messenger.sendRemoveModel(modelId);
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to remove model.");
        return;
      }
      await this.refreshState(null, "Model removed.");
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async handleSetProviderKey(
    envName: string,
    value: string,
  ): Promise<void> {
    try {
      const capabilities = this.buildCapabilities(
        await this.deps.ensureInitialized(),
      );
      if (!capabilities.setProviderKey) {
        await this.refreshState(
          "API key storage is unavailable for this serve instance.",
        );
        return;
      }
      const response = await this.deps.messenger.sendSetProviderKey(
        envName,
        value,
      );
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to store API key.");
        return;
      }
      await this.refreshState(null, `Saved ${envName}.`);
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async refreshState(
    error: string | null = null,
    status: string | null = null,
    warnings: string[] | null = null,
  ): Promise<void> {
    const initializeResult = await this.deps.ensureInitialized();
    const capabilities = this.buildCapabilities(initializeResult);
    const providerKeysResult = capabilities.listProviderKeys
      ? await this.fetchProviderKeys(this.state.providerKeys)
      : { error: null, providerKeys: [] };
    const modelsResult = capabilities.listModels
      ? await this.fetchModels(this.state.models)
      : { error: null, models: [] };
    const connectorsResult = this.route === "connectors" && capabilities.connectorCapabilities?.list
      ? await this.fetchConnectors(
        this.state.connectors ?? [],
        this.state.connectorConfigPaths,
      )
      : {
        error: null,
        connectors: this.state.connectors ?? [],
        configPaths: this.state.connectorConfigPaths,
      };
    this.state = {
      capabilities,
      error: error ?? modelsResult.error ?? providerKeysResult.error ?? connectorsResult.error,
      expectedCliVersion: this.deps.expectedCliVersion,
      extensionVersion: this.deps.extensionVersion,
      models: modelsResult.models,
      providerKeys: providerKeysResult.providerKeys,
      connectors: connectorsResult.connectors,
      connectorConfigPaths: connectorsResult.configPaths,
      ready: true,
      route: this.route,
      serverVersion: initializeResult.serverVersion,
      status,
      warnings,
    };
    this.postState();
  }

  private async refreshProviderKeys(
    error: string | null = null,
    status: string | null = null,
  ): Promise<void> {
    const initializeResult = await this.deps.ensureInitialized();
    const capabilities = this.buildCapabilities(initializeResult);
    const providerKeysResult = capabilities.listProviderKeys
      ? await this.fetchProviderKeys(this.state.providerKeys)
      : { error: null, providerKeys: [] };
    this.state = {
      ...this.state,
      capabilities,
      error: error ?? providerKeysResult.error,
      expectedCliVersion: this.deps.expectedCliVersion,
      extensionVersion: this.deps.extensionVersion,
      providerKeys: providerKeysResult.providerKeys,
      ready: true,
      route: this.route,
      serverVersion: initializeResult.serverVersion,
      status,
      warnings: null,
    };
    this.postState();
  }

  private async fetchModels(
    fallback: SettingsModelView[],
  ): Promise<{ error: string | null; models: SettingsModelView[] }> {
    try {
      const response = await this.deps.messenger.sendListModels();
      if (!response.success) {
        return {
          error: response.error ?? "Unable to load models.",
          models: fallback,
        };
      }
      return {
        error: null,
        models: parseModelsPayload(response.payload),
      };
    } catch (error) {
      return {
        error: String(error),
        models: fallback,
      };
    }
  }

  private async fetchConnectors(
    fallback: ConnectorView[],
    fallbackConfigPaths?: ConnectorConfigPaths,
  ): Promise<{
    error: string | null;
    connectors: ConnectorView[];
    configPaths?: ConnectorConfigPaths;
  }> {
    try {
      const response = await this.deps.messenger.sendListConnectors();
      if (response.success) {
        const parsed = parseConnectorsPayload(response.payload);
        return { error: null, ...parsed };
      }
      return {
        error: response.error ?? "Unable to load connectors.",
        connectors: fallback,
        configPaths: fallbackConfigPaths,
      };
    } catch (error) {
      return { error: String(error), connectors: fallback, configPaths: fallbackConfigPaths };
    }
  }

  private async fetchProviderKeys(    fallback: SettingsProviderKeyView[],
  ): Promise<{
    error: string | null;
    providerKeys: SettingsProviderKeyView[];
  }> {
    try {
      const response = await this.deps.messenger.sendListProviderKeys();
      if (!response.success) {
        return {
          error: response.error ?? "Unable to load provider keys.",
          providerKeys: fallback,
        };
      }
      return {
        error: null,
        providerKeys: parseProviderKeysPayload(response.payload),
      };
    } catch (error) {
      return {
        error: String(error),
        providerKeys: fallback,
      };
    }
  }

  private buildCapabilities(
    initializeResult: InitializeResult,
  ): SettingsCapabilities {
    return {
      listModels: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_LIST_MODELS,
      ),
      listProviderKeys: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_LIST_PROVIDER_KEYS,
      ),
      removeModel: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_REMOVE_MODEL,
      ),
      setProviderKey: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_SET_PROVIDER_KEY,
      ),
      upsertModel: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_UPSERT_MODEL,
      ),
      connectorCapabilities: {
        add: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.add),
        filter: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.filter),
        list: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.list),
        listTools: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.listTools),
        login: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.login),
        reload: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.reload),
        remove: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.remove),
        trust: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.trust),
      },
    };
  }

  private postState(): void {
    if (!this.panel) {
      return;
    }
    const frame: SettingsHostFrame = {
      channel: "state",
      content: this.state,
      messageId: `settings-state-${Date.now()}`,
    };
    void this.panel.webview.postMessage(frame);
  }

  private renderHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.deps.extensionUri.fsPath, "gui", "dist");
    const assets = resolveWebviewEntryAssets(
      distRoot,
      "settings.html",
      "settings.js",
    );
    if (assets.scripts.length === 0) {
      return this.renderFallbackHtml(
        "Tomcat settings assets are missing. Run `npm run build` in `tomcat-vscode-ext` first.",
      );
    }
    const nonce = getNonce();
    const styleTags = assets.stylesheets
      .map(
        (file) =>
          `<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}" />`,
      )
      .join("\n    ");
    const scriptTags = assets.scripts
      .map(
        (file) =>
          `<script nonce="${nonce}" type="module" src="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}"></script>`,
      )
      .join("\n    ");
    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta
      http-equiv="Content-Security-Policy"
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';"
    />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    ${styleTags}
    <title>Tomcat Settings</title>
  </head>
  <body>
    <div id="root"></div>
    ${scriptTags}
  </body>
</html>`;
  }

  private renderFallbackHtml(message: string): string {
    return `<!DOCTYPE html>
<html lang="en">
  <body>
    <pre>${message}</pre>
  </body>
</html>`;
  }
}
