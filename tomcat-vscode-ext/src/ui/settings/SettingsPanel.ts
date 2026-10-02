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
  SettingsConnectorReceipt,
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelRemovalReceipt,
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
  ConnectorProject,
  ConnectorToolFilter,
  ConnectorToolView,
  ConnectorView,
} from "../../shared/connectorsProtocol";
import {
  CONNECTOR_PROTOCOL_MISMATCH,
  normalizeConnectorView,
  parseConnectorProject,
  parseProjectTrustPayload,
  parseConnectorToolCatalog,
  parseSetConnectorToolEnabledResponse,
} from "../../shared/connectorsProtocol";
import { ConnectorReloadTracker } from "./ConnectorReloadTracker";

const CONNECTOR_CAPABILITIES = {
  add: "add_connector",
  filter: "set_connector_tool_filter",
  toggle: "set_connector_tool_enabled",
  list: "list_connectors",
  listTools: "list_connector_tools",
  login: "login_connector",
  reload: "reload_connector",
  remove: "remove_connector",
  trustProject: "trust_project",
} as const;

function getNonce(): string {
  return (
    Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
function connectorToolToggleKey(configKey: string, rawName: string): string {
  return JSON.stringify([configKey, rawName]);
}
function toolFilterSignature(connector: ConnectorView | undefined): string {
  return JSON.stringify({
    exclude: connector?.toolFilter?.exclude ?? [],
    include: connector?.toolFilter?.include ?? [],
  });
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
    supportedSpeeds: value.supportedSpeeds ?? [],
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
  return global ? { global, ...(workspace ? { workspace } : {}) } : undefined;
}

function parseConnectorsPayload(payload: unknown): {
  connectors: ConnectorView[];
  project: ConnectorProject | null;
  configPaths?: ConnectorConfigPaths;
} {
  if (!isRecord(payload) || !Array.isArray(payload.connectors)) throw new Error(CONNECTOR_PROTOCOL_MISMATCH);
  return {
    connectors: payload.connectors
      .map(normalizeConnectorView)
      .filter((connector): connector is ConnectorView => connector !== null),
    configPaths: parseConnectorConfigPaths(payload),
    project: parseConnectorProject(payload.project ?? null),
  };
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
    supportedSpeeds: model.supportedSpeeds ?? null,
    thinkingFormat: model.thinkingFormat ?? null,
  };
}

export interface SettingsPanelDeps {
  ensureInitialized(): Promise<InitializeResult>;
  expectedCliVersion: string | null;
  extensionUri: vscode.Uri;
  extensionVersion: string | null;
  messenger: TomcatMessenger;
  /** Clear the global Build preference if it points to a model being removed. */
  clearBuildModelPreference?(modelId: string): Promise<boolean | void> | boolean | void;
  onModelCatalogChanged?(): Promise<void> | void;
  /** The extension host, not a webview/editor heuristic, owns workspace choice. */
  selectConnectorWorkspaceRoot?(): Promise<string | null>;
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

type ConnectorToolToggleLock = {
  requestId: string;
  rawName: string;
  enabled: boolean;
};

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
  private connectorRefreshInterval = 0;
  private connectorEpoch = 0;
  private connectorReadSequence = 0;
  private connectorToolsReadSequence = 0;
  private connectorRefreshPending?: Promise<void>;
  private connectorRefreshAgain = false;
  private connectorToolsRequested?: string;
  private readonly connectorReloads = new ConnectorReloadTracker();
  private readonly connectorToolToggleLocks = new Map<string, ConnectorToolToggleLock>();
  private projectTrustPending = false;
  private readonly subscriptions: Array<{ dispose(): void }> = [];
  private connectorContextValue?: { workspaceRoot?: string | null };
  private connectorContextSelection?: Promise<{ workspaceRoot?: string | null }>;
  private viewEpoch = 0;
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

  constructor(private readonly deps: SettingsPanelDeps) {
    const exit = deps.messenger.onExit?.(() => {
      this.connectorEpoch += 1;
      this.viewEpoch += 1;
      this.connectorToolsReadSequence += 1;
      this.connectorToolsRequested = undefined;
      this.connectorContextSelection = undefined;
      this.connectorReloads.disconnected();
      const connectorToolToggles = { ...this.state.connectorToolToggles };
      for (const [configKey, operation] of this.connectorToolToggleLocks) {
        connectorToolToggles[connectorToolToggleKey(configKey, operation.rawName)] = {
          requestId: operation.requestId,
          configKey,
          rawName: operation.rawName,
          enabled: operation.enabled,
          error: "Connection lost; result unknown.",
        };
      }
      this.connectorToolToggleLocks.clear();
      this.state = { ...this.state, ready: false, error: "Connection lost.", connectorTools: [], connectorToolsIdentity: null, connectorReloads: this.connectorReloads.snapshot(), connectorToolToggles };
      this.postState();
      this.updateConnectorPoller();
    });
    if (exit) this.subscriptions.push(exit);
    const workspace = vscode.workspace.onDidChangeWorkspaceFolders?.(() => {
      this.resetConnectorViewLifecycle();
      if (this.route === "connectors" && this.webviewReady) void this.refreshState();
    });
    if (workspace) this.subscriptions.push(workspace);
  }

  onProjectTrusted(): void {
    if (this.route === "connectors" && this.webviewReady) void this.refreshConnectors(true);
  }

  private setRoute(route: SettingsRoute): void {
    if (route !== this.route) this.resetConnectorViewLifecycle();
    this.route = route;
    this.state = { ...this.state, route };
    this.updateConnectorPoller();
    // Navigation must not wait for model/key discovery: the old page may have
    // different controls and otherwise stays interactive with the wrong route.
    this.postState();
  }

  private shouldPreserveFocus(): boolean {
    return process.env.TOMCAT_E2E_SCREENSHOT !== "1";
  }

  private async connectorContext(): Promise<{ workspaceRoot?: string | null }> {
    if (this.connectorContextValue) {
      return this.connectorContextValue;
    }
    if (!this.connectorContextSelection) {
      const epoch = this.connectorEpoch;
      this.connectorContextSelection = (async () => {
        const workspaceRoot = this.deps.selectConnectorWorkspaceRoot
          ? await this.deps.selectConnectorWorkspaceRoot()
          : await this.selectDefaultConnectorWorkspaceRoot();
        if (epoch !== this.connectorEpoch) throw new Error("Connector workspace selection was superseded.");
        const context = { workspaceRoot };
        this.connectorContextValue = context;
        return context;
      })();
    }
    return this.connectorContextSelection;
  }

  private async selectDefaultConnectorWorkspaceRoot(): Promise<string | null> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    if (folders.length === 0) {
      return null;
    }
    if (folders.length === 1) {
      return folders[0].uri.fsPath;
    }
    const selected = await vscode.window.showWorkspaceFolderPick({
      placeHolder: "Select the workspace for Tomcat connector settings",
    });
    return selected?.uri.fsPath ?? null;
  }

  private resetConnectorViewLifecycle(): void {
    if (this.connectorRefreshTimer) {
      clearInterval(this.connectorRefreshTimer);
      this.connectorRefreshTimer = undefined;
    }
    // A closed settings panel must not retain a project chosen for a prior view.
    // The next view explicitly asks the Host again, including in multi-root workspaces.
    this.connectorContextValue = undefined;
    this.connectorContextSelection = undefined;
    this.viewEpoch += 1;
    this.connectorEpoch += 1;
    this.connectorReadSequence += 1;
    this.connectorToolsReadSequence += 1;
    this.connectorToolsRequested = undefined;
    this.connectorRefreshInterval = 0;
    this.connectorRefreshAgain = false;
    this.connectorReloads.clear();
    this.connectorToolToggleLocks.clear();
    this.projectTrustPending = false;
    this.state = { ...this.state, connectors: [], connectorProject: null, connectorTrustPending: false, connectorConfigPaths: undefined, connectorTools: [], connectorToolsIdentity: null, selectedConnector: null, connectorReloads: {}, connectorToolToggles: {}, connectorReceipt: null };
  }

  dispose(): void {
    this.webviewReady = false;
    for (const subscription of this.subscriptions.splice(0)) subscription.dispose();
    this.resetConnectorViewLifecycle();
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
    this.setRoute(route);
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
      this.resetConnectorViewLifecycle();
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
        this.setRoute(intent.data?.route ?? this.route);
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
      case "listConnectorTools":
        this.state = { ...this.state, selectedConnector: intent.data.configKey };
        await this.loadConnectorTools(intent.data.configKey, true);
        return;
      case "reloadConnector":
        await this.handleConnectorReload(intent.data.configKey, intent.messageId);
        return;
      case "addConnector":
        await this.handleAddConnector(intent.data.connector, intent.messageId, intent.data.trustProject === true);
        return;
      case "trustProject":
        await this.handleTrustProject(intent.data.projectRoot);
        return;
      case "removeConnector":
      case "loginConnector":
      case "cancelLoginConnector":
      case "logoutConnector":
        await this.handleConnectorAction(intent.type, intent.data.configKey);
        return;
      case "openConnectorConfig":
        await this.openConnectorConfig(intent.data.configKey, intent.data.scope);
        return;
      case "setConnectorToolFilter": {
        const epoch = this.connectorEpoch;
        try {
          const context = await this.connectorContext();
          if (epoch !== this.connectorEpoch) return;
          const response = await this.deps.messenger.sendSetConnectorToolFilter(
            intent.data.configKey, intent.data.filter.include, intent.data.filter.exclude, context,
          );
          if (epoch !== this.connectorEpoch) return;
          this.connectorToolsReadSequence += 1;
          this.connectorToolsRequested = undefined;
          this.state = { ...this.state, connectorTools: [], connectorToolsIdentity: null,
            error: response.success ? null : response.error ?? "Unable to update connector tools.",
            status: response.success ? "Connector tools updated." : null };
          this.postState();
          await this.refreshConnectors(true);
        } catch (error) {
          if (epoch !== this.connectorEpoch) return;
          this.state = { ...this.state, error: String(error) };
          this.postState();
        }
        return;
      }      case "setConnectorToolEnabled":
        await this.handleConnectorToolEnabled(
          intent.data.configKey,
          intent.data.rawName,
          intent.data.enabled,
          intent.messageId,
        );
        return;

    }
  }

  private async handleConnectorToolEnabled(
    configKey: string,
    rawName: string,
    enabled: boolean,
    requestId: string,
  ): Promise<void> {
    const epoch = this.connectorEpoch;
    if (this.state.capabilities.connectorCapabilities?.toggle !== true) {
      const error = "This Tomcat Serve does not support changing individual connector tools.";
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: {
            requestId,
            configKey,
            rawName,
            enabled,
            configSaved: false,
            runtimeApplied: false,
            error,
          },
        },
        error,
      };
      this.postState();
      return;
    }
    const source = this.state.connectors?.find((entry) => entry.configKey === configKey);
    if (!source || source.overridden || source.state !== "connected" || !this.connectorContextValue) {
      const error = "This connector is not available to change tools.";
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: {
            requestId,
            configKey,
            rawName,
            enabled,
            configSaved: false,
            runtimeApplied: false,
            error,
          },
        },
        error,
      };
      this.postState();
      return;
    }
    const reload = this.state.connectorReloads?.[configKey];
    if (reload?.phase === "pending" || reload?.phase === "accepted") {
      const error = "Wait for this connector to finish reloading before changing a tool.";
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: {
            requestId,
            configKey,
            rawName,
            enabled,
            configSaved: false,
            runtimeApplied: false,
            error,
          },
        },
        error,
      };
      this.postState();
      return;
    }
    if (this.connectorToolToggleLocks.has(configKey)) {
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: {
            requestId,
            configKey,
            rawName,
            enabled,
            configSaved: false,
            runtimeApplied: false,
            error: "Another tool setting for this connector is still being saved.",
          },
        },
      };
      this.postState();
      return;
    }
    // Any list that began before this write cannot authoritatively replace a
    // matching receipt. The next ordinary refresh will obtain a new catalog.
    this.connectorReadSequence += 1;
    this.connectorToolsReadSequence += 1;
    this.connectorToolsRequested = undefined;
    this.connectorToolToggleLocks.set(configKey, { requestId, rawName, enabled });
    try {
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch) return;
      const response = await this.deps.messenger.sendSetConnectorToolEnabled(
        configKey,
        rawName,
        enabled,
        context,
      );
      if (epoch !== this.connectorEpoch) return;
      const payload = parseSetConnectorToolEnabledResponse(response.payload, {
        configKey,
        rawName,
        enabled,
      });
      const { configSaved, runtimeApplied } = payload;
      const completed = response.success && configSaved && runtimeApplied;
      const rejected = !response.success && !configSaved && !runtimeApplied;
      const partial = !response.success && configSaved && !runtimeApplied;
      if (!completed && !rejected && !partial) {
        throw new Error(CONNECTOR_PROTOCOL_MISMATCH);
      }
      const action = enabled ? "enabled" : "disabled";
      const error = completed
        ? null
        : response.error ?? (partial
          ? "Runtime synchronization could not be confirmed."
          : "Unable to update this tool setting.");
      if (completed) this.confirmConnectorToolEnabled(configKey, rawName, enabled);
      const receipt = {
        requestId,
        configKey,
        rawName,
        enabled,
        configSaved,
        runtimeApplied,
        error,
      };
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: receipt,
        },
        error,
        status: completed
          ? `Tool ${action}.`
          : partial
            ? `Tool setting saved, but runtime synchronization could not be confirmed. Retry to synchronize. ${error ?? ""}`.trim()
            : error,
      };
      this.postState();
      // A durable write may need a later retry of cache synchronization; never
      // replay the mutation itself merely because this auxiliary refresh fails.
      if (configSaved) void this.refreshConnectors(true);
    } catch (error) {
      if (epoch !== this.connectorEpoch) return;
      const message = String(error);
      this.state = {
        ...this.state,
        connectorToolToggles: {
          ...this.state.connectorToolToggles,
          [connectorToolToggleKey(configKey, rawName)]: {
            requestId,
            configKey,
            rawName,
            enabled,
            configSaved: undefined,
            runtimeApplied: undefined,
            error: message,
          },
        },
        error: message,
      };
      this.postState();
      // A missing/invalid response is not evidence that the write failed. Use
      // the normal read path only; it never replays this mutation.
      void this.refreshConnectors(true);
    } finally {
      if (this.connectorToolToggleLocks.get(configKey)?.requestId === requestId) {
        this.connectorToolToggleLocks.delete(configKey);
      }
    }
  }

  /** Apply a full matching receipt only to the directory it was sent from. */
  private confirmConnectorToolEnabled(
    configKey: string,
    rawName: string,
    enabled: boolean,
  ): void {
    const connector = this.state.connectors?.find((entry) => entry.configKey === configKey);
    const identity = this.state.connectorToolsIdentity;
    if (!connector || connector.state !== "connected" || !identity
      || identity.configKey !== configKey || identity.generation !== connector.generation
      || identity.attempt !== connector.attempt) return;
    const tools = this.state.connectorTools ?? [];
    if (!tools.some((tool) => tool.rawName === rawName)) return;
    const nextTools = tools.map((tool) => tool.rawName === rawName ? { ...tool, enabled } : tool);
    const toolCount = nextTools.filter((tool) => tool.enabled).length;
    this.state = {
      ...this.state,
      connectorTools: nextTools,
      connectors: this.state.connectors?.map((entry) => entry.configKey === configKey
        ? { ...entry, toolCount }
        : entry),
    };
  }

  private async openConnectorConfig(
    configKey?: string,
    scope?: "global" | "workspace",
  ): Promise<void> {
    const connector = configKey
      ? this.state.connectors?.find((entry) => entry.configKey === configKey)
      : undefined;
    const scopedConfigPath = scope === "global"
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

  private async handleTrustProject(projectRoot: string): Promise<void> {
    if (this.projectTrustPending) return;
    const epoch = this.connectorEpoch;
    if (this.state.connectorProject?.root !== projectRoot
      || this.state.connectorProject.trusted
      || this.state.capabilities.connectorCapabilities?.trustProject !== true) {
      this.state = { ...this.state, error: "Project trust status changed. Refresh Connectors and retry." };
      this.postState();
      return;
    }
    this.projectTrustPending = true;
    this.state = { ...this.state, connectorTrustPending: true };
    this.postState();
    try {
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch) return;
      if (!context.workspaceRoot || this.state.connectorProject?.root !== projectRoot) {
        throw new Error("Project selection changed. Refresh Connectors and retry.");
      }
      const response = await this.deps.messenger.sendTrustProject(projectRoot);
      if (epoch !== this.connectorEpoch) return;
      if (!response.success) throw new Error(response.error ?? "Unable to trust project.");
      const result = parseProjectTrustPayload(response.payload);
      if (!result.trusted || result.projectRoot !== projectRoot || result.error) {
        throw new Error(CONNECTOR_PROTOCOL_MISMATCH);
      }
      this.state = { ...this.state, error: null, status: "Project trusted. Connecting services…" };
      await this.refreshConnectors(true);
    } catch (error) {
      if (epoch === this.connectorEpoch) {
        this.state = { ...this.state, error: String(error) };
      }
    } finally {
      this.projectTrustPending = false;
      if (epoch === this.connectorEpoch) {
        this.state = { ...this.state, connectorTrustPending: false };
        this.postState();
      }
    }
  }

  private async handleAddConnector(
    input: ConnectorInput,
    requestId: string,
    trustProject: boolean,
  ): Promise<void> {
    const epoch = this.connectorEpoch;
    const failed = (error: string): void => {
      if (epoch !== this.connectorEpoch) return;
      const receipt: SettingsConnectorReceipt = {
        configSaved: false,
        connectionStarted: false,
        error,
        name: input.name,
        requestId,
      };
      this.state = {
        ...this.state,
        connectorReceipt: receipt,
        error,
        status: "Connector add failed.",
      };
      this.postState();
      void this.refreshState(error, "Connector add failed.", null, receipt);
    };
    try {
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch) return;
      if (trustProject && (input.scope !== "workspace" || !context.workspaceRoot
        || this.state.connectorProject?.trusted !== false
        || this.state.capabilities.connectorCapabilities?.trustProject !== true)) {
        failed("Project trust status changed. Refresh Connectors before adding.");
        return;
      }
      const response = await this.deps.messenger.sendAddConnector({
        args: input.args ?? [],
        command: input.command ?? "",
        auth: input.auth,
        env: input.env,
        headers: input.headers,
        name: input.name,
        oauth: input.oauth,
        context,
        scope: input.scope,
        ...(trustProject ? { trustProject: true } : {}),
        url: input.url,
      });
      if (epoch !== this.connectorEpoch) return;
      if (!response.success) {
        failed(response.error ?? "Unable to add connector.");
        return;
      }
      const payload = isRecord(response.payload) ? response.payload : {};
      const configSaved = payload.configSaved === true;
      const connectionStarted = payload.connectionStarted === true;
      const postSaveError = typeof payload.postSaveError === "string" ? payload.postSaveError : null;
      if (!configSaved) {
        failed(postSaveError ?? "Connector did not acknowledge configuration persistence.");
        return;
      }
      const receipt: SettingsConnectorReceipt = {
        configSaved,
        connectionStarted,
        error: postSaveError,
        name: input.name,
        requestId,
      };
      const status = connectionStarted
        ? "Connector saved. Connection is starting."
        : `Connector saved, but connection was not started.${postSaveError ? ` ${postSaveError}` : ""}`;
      this.state = {
        ...this.state,
        connectorReceipt: receipt,
        error: null,
        status,
      };
      this.postState();
      void this.refreshState(null, status, null, receipt);
    } catch (error) {
      failed(String(error));
    }
  }

  private publishConnectorReloads(): void {
    this.state = { ...this.state, connectorReloads: this.connectorReloads.snapshot() };
    this.postState();
    this.updateConnectorPoller();
  }

  private async handleConnectorReload(configKey: string, requestId: string): Promise<void> {
    if (this.connectorToolToggleLocks.has(configKey)) {
      const entry = this.connectorReloads.begin(configKey, requestId);
      if (entry) {
        this.connectorReloads.finish(
          entry,
          "failed",
          "Wait for the tool setting to finish before reloading this connector.",
          "rejected",
        );
        this.publishConnectorReloads();
      }
      return;
    }
    const entry = this.connectorReloads.begin(configKey, requestId);
    if (!entry) return;
    const epoch = this.connectorEpoch;
    if (this.state.selectedConnector === configKey) {
      this.connectorToolsReadSequence += 1;
      this.connectorToolsRequested = undefined;
      this.state = { ...this.state, connectorTools: [], connectorToolsIdentity: null };
    }
    this.publishConnectorReloads();
    try {
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch || !this.connectorReloads.active(entry)) return;
      const response = await this.deps.messenger.sendReloadConnector(configKey, context);
      if (epoch !== this.connectorEpoch || !this.connectorReloads.active(entry)) return;
      if (!response.success) {
        this.connectorReloads.finish(entry, "failed", response.error ?? "Reconnection was rejected.", "rejected");
      } else {
        this.connectorReloads.accept(entry, response.payload, this.connectorReadSequence, Date.now());
      }
    } catch (error) {
      if (epoch !== this.connectorEpoch) return;
      const incompatible = error instanceof Error && error.message === CONNECTOR_PROTOCOL_MISMATCH;
      this.connectorReloads.finish(entry, "unknown", incompatible ? CONNECTOR_PROTOCOL_MISMATCH : `Unable to confirm reconnection. ${String(error)}`, incompatible ? "incompatible" : "connection-lost");
    }
    if (epoch !== this.connectorEpoch) return;
    this.publishConnectorReloads();
    // Never turn a read failure into another Reload. A read started before the
    // acknowledgement cannot settle this receipt, even if it finishes later.
    void this.refreshConnectors(true);
  }

  private async handleConnectorAction(
    action: "removeConnector" | "loginConnector" | "logoutConnector" | "cancelLoginConnector",
    configKey: string,
  ): Promise<void> {
    const epoch = this.connectorEpoch;
    try {
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch) return;
      const response = action === "removeConnector"
        ? await this.deps.messenger.sendRemoveConnector(configKey, context)
        : action === "loginConnector"
          ? await this.deps.messenger.sendLoginConnector(configKey, context)
            : action === "cancelLoginConnector"
              ? await this.deps.messenger.sendCancelLoginConnector(configKey, context)
              : await this.deps.messenger.sendLogoutConnector(configKey, context);
      if (epoch !== this.connectorEpoch) return;
      this.state = { ...this.state, error: response.success ? null : response.error ?? "Connector operation failed.", status: response.success ? action === "loginConnector" ? "Authorizing connector…" : "Connector updated." : null };
      this.postState();
      await this.refreshConnectors(true);
    } catch (error) {
      if (epoch !== this.connectorEpoch) return;
      this.state = { ...this.state, error: String(error) };
      this.postState();
    }
  }

  private async handleRemoveModel(modelId: string): Promise<void> {
    let buildPreferenceCleared = false;
    const report = async (
      error: string | null,
      status: string | null,
      warnings: string[],
      success: boolean,
    ) => {
      const modelRemovalReceipt = { modelId, success, warnings };
      // Publish the authoritative operation result before an auxiliary catalog reload.
      // The form can stop its spinner even if that later read is slow or fails.
      this.state = {
        ...this.state,
        error,
        modelRemovalReceipt,
        status,
        warnings,
      };
      this.postState();
      await this.refreshState(
        error,
        status,
        warnings,
        null,
        modelRemovalReceipt,
      );
    };
    const failed = async (error: unknown) => {
      const warnings = buildPreferenceCleared
        ? ["The matching Build preference was cleared before deletion."]
        : [];
      await report(String(error), null, warnings, false);
    };
    try {
      const capabilities = this.buildCapabilities(
        await this.deps.ensureInitialized(),
      );
      if (!capabilities.removeModel) {
        await failed("Model removal is unavailable for this serve instance.");
        return;
      }
      // This write belongs to the extension host. Await it before asking serve to delete,
      // so a remembered Build choice cannot point to an ID that no longer exists.
      buildPreferenceCleared =
        (await this.deps.clearBuildModelPreference?.(modelId)) === true;
      this.state = {
        ...this.state,
        error: null,
        modelRemovalReceipt: null,
        status: `Removing ${modelId}…`,
        warnings: null,
      };
      this.postState();
      const response = await this.deps.messenger.sendRemoveModel(modelId);
      if (!response.success) {
        await failed(response.error ?? "Unable to remove model.");
        return;
      }
      const removalWarnings = (response.payload as { warnings?: unknown } | null)
        ?.warnings;
      const warnings = Array.isArray(removalWarnings)
        ? removalWarnings.filter((warning): warning is string => typeof warning === "string")
        : [];
      await report(
        null,
        warnings.length > 0 ? "Model removed with warnings." : "Model removed.",
        warnings,
        true,
      );
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await failed(error);
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
    connectorReceipt: SettingsConnectorReceipt | null = null,
    modelRemovalReceipt: SettingsModelRemovalReceipt | null = null,
  ): Promise<void> {
    if (this.route === "connectors") {
      this.state = { ...this.state, error: error ?? this.state.error, status: status ?? this.state.status, connectorReceipt: connectorReceipt ?? this.state.connectorReceipt };
      this.postState();
      await this.refreshConnectors(true);
      return;
    }
    const viewEpoch = ++this.viewEpoch;
    const initializeResult = await this.deps.ensureInitialized();
    const capabilities = this.buildCapabilities(initializeResult);
    const providerKeysResult = capabilities.listProviderKeys
      ? await this.fetchProviderKeys(this.state.providerKeys)
      : { error: null, providerKeys: [] };
    const modelsResult = capabilities.listModels
      ? await this.fetchModels(this.state.models)
      : { error: null, models: [] };
    if (viewEpoch !== this.viewEpoch) {
      return;
    }
    this.state = {
      ...this.state,
      capabilities,
      error: error ?? modelsResult.error ?? providerKeysResult.error,
      expectedCliVersion: this.deps.expectedCliVersion,
      extensionVersion: this.deps.extensionVersion,
      models: modelsResult.models,
      modelRemovalReceipt,
      providerKeys: providerKeysResult.providerKeys,
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

  private updateConnectorPoller(): void {
    const interval = !this.webviewReady || this.route !== "connectors" ? 0
      : this.connectorReloads.busy || this.state.connectors?.some((entry) => entry.state === "connecting") ? 1000 : 5000;
    if (interval === this.connectorRefreshInterval) return;
    if (this.connectorRefreshTimer) clearInterval(this.connectorRefreshTimer);
    this.connectorRefreshTimer = undefined;
    this.connectorRefreshInterval = interval;
    if (interval) this.connectorRefreshTimer = setInterval(() => {
      if (this.connectorReloads.expire(Date.now())) this.publishConnectorReloads();
      void this.refreshConnectors();
    }, interval);
  }

  private async refreshConnectors(afterCurrent = false): Promise<void> {
    if (this.connectorRefreshPending) {
      if (afterCurrent) this.connectorRefreshAgain = true;
      return this.connectorRefreshPending;
    }
    const run = (async () => {
      do {
        this.connectorRefreshAgain = false;
        await this.refreshConnectorSnapshot();
      } while (this.connectorRefreshAgain && this.route === "connectors");
    })();
    this.connectorRefreshPending = run;
    try { await run; }
    finally {
      if (this.connectorRefreshPending === run) this.connectorRefreshPending = undefined;
      this.updateConnectorPoller();
    }
  }

  private async refreshConnectorSnapshot(): Promise<void> {
    const epoch = this.connectorEpoch;
    try {
      const initialized = await this.deps.ensureInitialized();
      const capabilities = this.buildCapabilities(initialized);
      if (epoch !== this.connectorEpoch || this.route !== "connectors") return;
      if (!capabilities.connectorCapabilities?.list) {
        this.state = { ...this.state, capabilities, ready: true };
        this.postState();
        return;
      }
      const context = await this.connectorContext();
      if (epoch !== this.connectorEpoch) return;
      const read = ++this.connectorReadSequence;
      const response = await this.deps.messenger.sendListConnectors(context);
      if (epoch !== this.connectorEpoch || read !== this.connectorReadSequence) return;
      if (!response.success) throw new Error(response.error ?? "Unable to load connectors.");
      const parsed = parseConnectorsPayload(response.payload);
      const previous = this.state.connectors?.find((entry) => entry.configKey === this.state.selectedConnector);
      this.connectorReloads.expire(Date.now());
      this.connectorReloads.observe(parsed.connectors, read);
      this.state = {
        ...this.state, capabilities, ready: true,
        error: isRecord(response.payload) && typeof response.payload.error === "string" ? response.payload.error : null,
        expectedCliVersion: this.deps.expectedCliVersion, extensionVersion: this.deps.extensionVersion,
        serverVersion: initialized.serverVersion, connectors: parsed.connectors,
        connectorProject: parsed.project,
        connectorConfigPaths: parsed.configPaths, connectorReloads: this.connectorReloads.snapshot(),
      };
      const selected = parsed.connectors.find((entry) => entry.configKey === this.state.selectedConnector);
      const click = selected && this.state.connectorReloads?.[selected.configKey];
      const waiting = click?.phase === "pending" || click?.phase === "accepted";
      const filterChanged = Boolean(previous && selected
        && toolFilterSignature(previous) !== toolFilterSignature(selected));
      const identityChanged = !selected || selected.state !== "connected" || waiting
        || previous?.generation !== selected.generation || previous?.attempt !== selected.attempt;
      if (identityChanged) {
        this.connectorToolsReadSequence += 1;
        this.connectorToolsRequested = undefined;
        this.state = { ...this.state, selectedConnector: selected?.configKey ?? null, connectorTools: [], connectorToolsIdentity: null };
      } else if (filterChanged) {
        // A filter-only refresh must not erase the management catalog before its
        // replacement arrives. Its response still gets a new read sequence.
        this.connectorToolsReadSequence += 1;
        this.connectorToolsRequested = undefined;
      }
      this.postState();
      if (selected?.state === "connected" && !waiting) void this.loadConnectorTools(selected.configKey);
    } catch (error) {
      if (epoch !== this.connectorEpoch) return;
      // A failed list is not evidence that recovery failed. Keep its receipt.
      this.state = { ...this.state, error: String(error), ready: true };
      this.postState();
    }
  }

  private async loadConnectorTools(configKey: string, force = false): Promise<void> {
    const connector = this.state.connectors?.find((entry) => entry.configKey === configKey);
    const epoch = this.connectorEpoch;
    const click = this.state.connectorReloads?.[configKey];
    if (!connector || connector.state !== "connected" || click?.phase === "pending" || click?.phase === "accepted") {
      this.state = { ...this.state, connectorTools: [], connectorToolsIdentity: null };
      this.postState();
      return;
    }
    const identity = JSON.stringify([epoch, configKey, connector.generation, connector.attempt]);
    if (!force && identity === this.connectorToolsRequested) return;
    this.connectorToolsRequested = identity;
    const read = ++this.connectorToolsReadSequence;
    const current = (): boolean => epoch === this.connectorEpoch && read === this.connectorToolsReadSequence && this.state.selectedConnector === configKey;
    try {
      if (connector.compatibilityError) throw new Error(connector.compatibilityError);
      const context = await this.connectorContext();
      if (!current()) return;
      const response = await this.deps.messenger.sendListConnectorTools(configKey, context);
      if (!current()) return;
      if (!response.success) throw new Error(response.error ?? "Unable to load connector tools.");
      const catalog = parseConnectorToolCatalog(response.payload, configKey);
      const latest = this.state.connectors?.find((entry) => entry.configKey === configKey);
      if (latest?.state !== "connected" || latest.generation !== catalog.generation || latest.attempt !== catalog.attempt) {
        // Retry on the existing polling interval, not a hot read/refresh loop
        // if the remote catalog repeatedly carries inconsistent metadata.
        this.connectorToolsRequested = undefined;
        return;
      }
      this.state = { ...this.state, connectorTools: catalog.tools, connectorToolsIdentity: { configKey, generation: catalog.generation, attempt: catalog.attempt }, error: null };
      this.postState();
    } catch (error) {
      if (!current()) return;
      // Deduplicate successful/in-flight reads, not a failed directory lookup.
      // The existing poller may retry this read; it must never issue Reload.
      this.connectorToolsRequested = undefined;
      // A failed read is not proof the source has no tools. Keep the last
      // management catalog and let the existing poller retry without Reload.
      this.state = { ...this.state, error: String(error) };
      this.postState();
    }
  }

  private async fetchProviderKeys(
    fallback: SettingsProviderKeyView[],
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
        toggle: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.toggle),
        list: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.list),
        listTools: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.listTools),
        login: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.login),
        reload: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.reload),
        remove: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.remove),
        trustProject: hasServeCapability(initializeResult, CONNECTOR_CAPABILITIES.trustProject),
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
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource}; script-src ${webview.cspSource} 'nonce-${nonce}';"
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
