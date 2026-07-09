export type SettingsRoute = "models";

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
  id: string;
  keyPresent: boolean;
  modelName?: string | null;
  provider: string;
  source: SettingsModelSource;
  thinkingFormat?: string | null;
}

export interface SettingsModelInput {
  api: string;
  apiKeyEnv?: string | null;
  baseUrl?: string | null;
  capabilities: SettingsModelCapabilities;
  contextWindow?: number | null;
  id: string;
  modelName?: string | null;
  provider: string;
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

export interface SettingsCapabilities {
  listModels: boolean;
  listProviderKeys: boolean;
  removeModel: boolean;
  setProviderKey: boolean;
  upsertModel: boolean;
}

export interface SettingsStateSnapshot {
  capabilities: SettingsCapabilities;
  error?: string | null;
  models: SettingsModelView[];
  providerKeys: SettingsProviderKeyView[];
  ready: boolean;
  route: SettingsRoute;
  status?: string | null;
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
  return value === "models";
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
    typeof value.id === "string" &&
    (value.modelName === undefined || value.modelName === null || typeof value.modelName === "string") &&
    typeof value.provider === "string" &&
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
    default:
      return false;
  }
}
