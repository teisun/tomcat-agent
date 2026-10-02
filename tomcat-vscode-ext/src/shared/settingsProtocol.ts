import type {
  ConnectorConfigPaths,
  ConnectorConnectionIdentity,
  ConnectorInput,
  ConnectorProject,
  ConnectorToolFilter,
  ConnectorToolView,
  ConnectorView,
} from "./connectorsProtocol";

import { isSpeed, type Speed } from "./modelSpeed";
export type SettingsRoute = "models" | "connectors";

export interface SettingsModelCapabilities {
  files: boolean;
  reasoning: boolean;
  tools: boolean;
  vision: boolean;
  webSearch: boolean;
}

export type SettingsModelSource = "builtin" | "user";

export interface SettingsModelView {
  api: string;
  apiKeyEnv: string;
  baseUrl?: string | null;
  capabilities: SettingsModelCapabilities;
  contextWindow?: number | null;
  contextWindowOptions?: number[] | null;
  description?: string | null;
  id: string;
  keyPresent: boolean;
  maxOutputTokens?: number | null;
  modelName?: string | null;
  provider: string;
  source: SettingsModelSource;
  supportedReasoningLevels?: string[] | null;
  supportedSpeeds?: Speed[] | null;
  thinkingFormat?: string | null;
}

export interface SettingsModelInput {
  api: string;
  apiKeyEnv?: string | null;
  baseUrl?: string | null;
  capabilities: SettingsModelCapabilities;
  contextWindow?: number | null;
  contextWindowOptions?: number[] | null;
  description?: string | null;
  id: string;
  maxOutputTokens?: number | null;
  modelName?: string | null;
  provider: string;
  supportedReasoningLevels?: string[] | null;
  supportedSpeeds?: Speed[] | null;
  thinkingFormat?: string | null;
}

export interface SettingsProviderKeyView {
  envName: string;
  keyPresent: boolean;
  modelIds: string[];
  provider: string;
}

export interface SettingsProviderKeyInput {
  envName: string;
  value: string;
}

export interface SettingsModelRemovalReceipt {
  modelId: string;
  success: boolean;
  warnings: string[];
}

export interface SettingsCapabilities {
  listModels: boolean;
  listProviderKeys: boolean;
  removeModel: boolean;
  setProviderKey: boolean;
  upsertModel: boolean;
  connectorCapabilities?: SettingsConnectorCapabilities;
}

export interface SettingsConnectorCapabilities {
  list: boolean;
  listTools: boolean;
  add: boolean;
  remove: boolean;
  reload: boolean;
  trustProject: boolean;
  filter: boolean;
  toggle: boolean;
  login: boolean;
}

export interface SettingsConnectorReceipt {
  requestId: string;
  configSaved: boolean;
  connectionStarted: boolean;
  error?: string | null;
  name?: string | null;
}

/** One latest click per source, not a second copy of backend connection state. */
export interface SettingsConnectorReloadReceipt {
  requestId: string;
  configKey: string;
  phase: "pending" | "accepted" | "succeeded" | "failed" | "unknown";
  generation?: string;
  message?: string;
  reason?: "superseded" | "removed" | "rejected" | "connection-lost" | "timeout" | "incompatible";
}
/** One durable setting result per source tool; it is not a second tool catalog. */
export interface SettingsConnectorToolToggleReceipt {
  requestId: string;
  configKey: string;
  rawName: string;
  enabled: boolean;
  configSaved?: boolean;
  runtimeApplied?: boolean;
  error?: string | null;
}


export interface SettingsStateSnapshot {
  capabilities: SettingsCapabilities;
  error?: string | null;
  expectedCliVersion?: string | null;
  extensionVersion?: string | null;
  models: SettingsModelView[];
  modelRemovalReceipt?: SettingsModelRemovalReceipt | null;
  providerKeys: SettingsProviderKeyView[];
  connectors?: ConnectorView[];
  connectorConfigPaths?: ConnectorConfigPaths;
  connectorProject?: ConnectorProject | null;
  connectorTrustPending?: boolean;
  connectorCapabilities?: SettingsConnectorCapabilities;
  connectorTools?: ConnectorToolView[];
  connectorReceipt?: SettingsConnectorReceipt | null;
  connectorReloads?: Record<string, SettingsConnectorReloadReceipt>;
  connectorToolToggles?: Record<string, SettingsConnectorToolToggleReceipt>;
  connectorToolsIdentity?: ConnectorConnectionIdentity | null;


  selectedConnector?: string | null;
  ready: boolean;
  route: SettingsRoute;
  serverVersion?: string | null;
  status?: string | null;
  warnings?: string[] | null;
}

export type SettingsHostFrame = {
  channel: "state";
  content: SettingsStateSnapshot;
  messageId: string;
};

export type SettingsIntent =
  | {
      messageId: string;
      type: "settings.ready";
      data?: {
        route?: SettingsRoute | null;
      };
    }
  | {
      messageId: string;
      type: "listModels";
    }
  | {
      messageId: string;
      type: "listProviderKeys";
    }
  | {
      messageId: string;
      type: "upsertModel";
      data: {
        model: SettingsModelInput;
        providerKey?: SettingsProviderKeyInput;
      };
    }
  | {
      messageId: string;
      type: "removeModel";
      data: {
        modelId: string;
      };
    }
  | {
      messageId: string;
      type: "setProviderKey";
      data: {
        envName: string;
        value: string;
      };
    }
  | {
      messageId: string;
      type: "listConnectors" | "reloadConnectors";
    }
  | {
      messageId: string;
      type: "listConnectorTools" | "reloadConnector" | "removeConnector" | "loginConnector" | "logoutConnector" | "cancelLoginConnector";
      data: { name: string; configKey: string };
    }
  | {
      messageId: string;
      type: "trustProject";
      data: { projectRoot: string };
    }
  | {
      messageId: string;
      type: "addConnector";
      data: { connector: ConnectorInput; trustProject?: true };
    }
  | {
      messageId: string;
      type: "setConnectorToolFilter";
      data: { name: string; configKey: string; filter: ConnectorToolFilter };
    }
  | {
      messageId: string;
      type: "setConnectorToolEnabled";
      data: { name: string; configKey: string; rawName: string; enabled: boolean };
    }
  | {
      messageId: string;
      type: "openConnectorConfig";
      data: { configKey?: string; scope?: "global" | "workspace" };
    };

export interface VsCodeApiLike<TMessage = unknown> {
  postMessage(message: TMessage): void;
  setState?(state: unknown): void;
}

export function acquireVsCodeApiLike<TMessage = unknown>(): VsCodeApiLike<TMessage> {
  const acquire = (globalThis as typeof globalThis & {
    acquireVsCodeApi?: () => VsCodeApiLike<TMessage>;
  }).acquireVsCodeApi;
  return acquire?.() ?? {
    postMessage() {},
    setState() {},
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isSettingsRoute(value: unknown): value is SettingsRoute {
  return value === "models" || value === "connectors";
}

function isSettingsModelCapabilities(value: unknown): value is SettingsModelCapabilities {
  return (
    isRecord(value) &&
    typeof value.files === "boolean" &&
    typeof value.reasoning === "boolean" &&
    typeof value.tools === "boolean" &&
    typeof value.vision === "boolean" &&
    typeof value.webSearch === "boolean"
  );
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

function isSettingsModelInput(value: unknown): value is SettingsModelInput {
  return (
    isRecord(value) &&
    typeof value.api === "string" &&
    (value.apiKeyEnv === undefined || value.apiKeyEnv === null || typeof value.apiKeyEnv === "string") &&
    (value.baseUrl === undefined || value.baseUrl === null || typeof value.baseUrl === "string") &&
    isSettingsModelCapabilities(value.capabilities) &&
    (value.contextWindow === undefined ||
      value.contextWindow === null ||
      typeof value.contextWindow === "number") &&
    (value.contextWindowOptions === undefined ||
      value.contextWindowOptions === null ||
      (Array.isArray(value.contextWindowOptions) &&
        value.contextWindowOptions.every((entry) => typeof entry === "number"))) &&
    (value.description === undefined || value.description === null || typeof value.description === "string") &&
    typeof value.id === "string" &&
    (value.maxOutputTokens === undefined ||
      value.maxOutputTokens === null ||
      typeof value.maxOutputTokens === "number") &&
    (value.modelName === undefined || value.modelName === null || typeof value.modelName === "string") &&
    typeof value.provider === "string" &&
    (value.supportedReasoningLevels === undefined ||
      value.supportedReasoningLevels === null ||
      isStringArray(value.supportedReasoningLevels)) &&
    (value.supportedSpeeds === undefined || value.supportedSpeeds === null ||
      (Array.isArray(value.supportedSpeeds) && value.supportedSpeeds.every(isSpeed))) &&
    (value.thinkingFormat === undefined ||
      value.thinkingFormat === null ||
      typeof value.thinkingFormat === "string")
  );
}

function isSettingsProviderKeyInput(value: unknown): value is SettingsProviderKeyInput {
  return isRecord(value) && typeof value.envName === "string" && typeof value.value === "string";
}

export function isSettingsIntent(value: unknown): value is SettingsIntent {
  if (!isRecord(value) || typeof value.messageId !== "string" || typeof value.type !== "string") {
    return false;
  }
  switch (value.type) {
    case "settings.ready":
      return (
        value.data === undefined ||
        (isRecord(value.data) &&
          (value.data.route === undefined || value.data.route === null || isSettingsRoute(value.data.route)))
      );
    case "listModels":
    case "listProviderKeys":
      return true;
    case "upsertModel":
      return (
        isRecord(value.data) &&
        isSettingsModelInput(value.data.model) &&
        (value.data.providerKey === undefined || isSettingsProviderKeyInput(value.data.providerKey))
      );
    case "removeModel":
      return isRecord(value.data) && typeof value.data.modelId === "string";
    case "setProviderKey":
      return (
        isRecord(value.data) &&
        typeof value.data.envName === "string" &&
        typeof value.data.value === "string"
      );
    case "listConnectors":
    case "reloadConnectors":
      return true;
    case "listConnectorTools":
    case "reloadConnector":
    case "removeConnector":
    case "loginConnector":
    case "logoutConnector":
    case "cancelLoginConnector":
      return isRecord(value.data) && typeof value.data.name === "string" && typeof value.data.configKey === "string";
    case "trustProject":
      return isRecord(value.data) && typeof value.data.projectRoot === "string" && value.data.projectRoot.length > 0;
    case "addConnector":
      return isRecord(value.data) && isRecord(value.data.connector) && typeof value.data.connector.name === "string"
        && (value.data.trustProject === undefined || value.data.trustProject === true);
    case "setConnectorToolFilter":
      return isRecord(value.data) && typeof value.data.name === "string" && typeof value.data.configKey === "string" && isRecord(value.data.filter);    case "setConnectorToolEnabled":
      return isRecord(value.data) && typeof value.data.name === "string" && typeof value.data.configKey === "string"
        && typeof value.data.rawName === "string" && typeof value.data.enabled === "boolean";

    case "openConnectorConfig":
      return (
        isRecord(value.data) &&
        (typeof value.data.configKey === "string" ||
          value.data.scope === "global" ||
          value.data.scope === "workspace")
      );

    default:
      return false;
  }
}
