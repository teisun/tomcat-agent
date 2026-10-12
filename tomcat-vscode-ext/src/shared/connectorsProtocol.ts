import { t } from "./i18n";

import type {
  ConnectorRecoveryProgress,
  ConnectorReloadReceipt,
  ListConnectorToolsPayload,
  ProjectTrustPayload,
  SetConnectorToolEnabledResponse,
} from "../serveClient/wire";
export type {
  ConnectorRecoveryProgress,
  ConnectorReloadReceipt,
  ListConnectorToolsPayload,
  ProjectTrustPayload,
  SetConnectorToolEnabledResponse,
};

export class ConnectorProtocolError extends Error {
  constructor() { super(t("connector.protocolMismatch")); }
}

export interface ConnectorConnectionIdentity {
  configKey: string;
  generation: string;
  attempt: number;
}

export type ConnectorType = "mcp" | "cli" | "a2a";
export type ConnectorTransport = "stdio" | "http";
export type ConnectorState =
  | "pending"
  | "connecting"
  | "connected"
  | "disconnected"
  | "awaiting_project_trust"
  | "needs_authorization"
  | "failed";
export type ConnectorScope = "global" | "workspace";

export interface ConnectorConfigPath {
  display: string;
  raw: string;
}

export interface ConnectorConfigPaths {
  global: ConnectorConfigPath;
  /** Undefined when this session has no explicit project root. */
  workspace?: ConnectorConfigPath;
}

export interface ConnectorProject {
  root: string;
  trusted: boolean;
}

/** Trust lookup can diagnose an unreadable record without granting access. */
export function parseProjectTrustPayload(value: unknown): ProjectTrustPayload {
  if (!isRecord(value) || typeof value.projectRoot !== "string" || !value.projectRoot
    || typeof value.trusted !== "boolean"
    || (value.error !== undefined && value.error !== null && typeof value.error !== "string")
    || (value.trusted && typeof value.error === "string")) {
    throw new ConnectorProtocolError();
  }
  return { projectRoot: value.projectRoot, trusted: value.trusted, error: value.error as string | null | undefined };
}

export function parseConnectorProject(value: unknown): ConnectorProject | null {
  if (value === null) return null;
  if (!isRecord(value) || typeof value.root !== "string" || !value.root || typeof value.trusted !== "boolean") {
    throw new ConnectorProtocolError();
  }
  return { root: value.root, trusted: value.trusted };
}

export interface ConnectorView {
  configKey: string;
  name: string;
  type: ConnectorType;
  transport: ConnectorTransport;
  source: ConnectorScope;
  /** The Global definition is visible for management but Workspace owns execution. */
  overridden: boolean;
  auth?: "none" | "bearer" | "oauth" | null;
  oauthConfigured: boolean;
  state: ConnectorState;
  /** Absent only for an incompatible Serve; never synthesize an identity. */
  generation?: string;
  attempt?: number;
  recovery?: ConnectorRecoveryProgress | null;
  compatibilityError?: string;
  toolCount: number;
  resourceCount: number;
  url?: string | null;
  command?: string | null;
  configPath?: string | null;
  configPathRaw?: string | null;
  error?: string | null;
  toolFilter?: ConnectorToolFilter;
}

export interface ConnectorToolView {
  modelName: string;
  rawName: string;
  label: string;
  description: string;
  inputSchema?: unknown;
  enabled: boolean;
}

export interface ConnectorToolFilter {
  include: string[];
  exclude: string[];
}

export interface ConnectorInput {
  name: string;
  type: "mcp";
  transport: ConnectorTransport;
  command?: string;
  args?: string[];
  url?: string;
  headers?: Record<string, string>;
  auth?: "none" | "bearer" | "oauth";
  env?: Record<string, string>;
  oauth?: {
    clientId?: string;
    scopes?: string[];
    callbackUrl?: string;
  };
  scope: ConnectorScope;
}

export interface ConnectorsHostFrame {
  connectors: ConnectorView[];
  project: ConnectorProject | null;
  selected?: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isConnectorGeneration(value: unknown): value is string {
  return typeof value === "string" && /^(0|[1-9][0-9]*)$/.test(value)
    && (value.length < 20 || (value.length === 20 && value <= "18446744073709551615"));
}

function isInteger(value: unknown, minimum: number, maximum = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}

function isRecoveryProgress(value: unknown): value is ConnectorRecoveryProgress {
  return isRecord(value) && typeof value.phase === "string" && value.phase.length > 0
    && isInteger(value.maxAttempts, 1, 3) && isInteger(value.remainingMs, 0);
}

export function parseConnectorReloadReceipt(value: unknown, configKey: string): ConnectorReloadReceipt {
  if (!isRecord(value) || value.configKey !== configKey || value.accepted !== true
    || !isConnectorGeneration(value.generation) || value.generation === "0"
    || !isInteger(value.recoveryTimeoutMs, 1, Number.MAX_SAFE_INTEGER - 10_000)) {
    throw new ConnectorProtocolError();
  }
  return { configKey, accepted: true, generation: value.generation, recoveryTimeoutMs: value.recoveryTimeoutMs };
}

export function parseConnectorToolCatalog(value: unknown, configKey: string): ListConnectorToolsPayload {
  if (!isRecord(value) || value.configKey !== configKey || !isConnectorGeneration(value.generation)
    || !isInteger(value.attempt, 1, 3) || !Array.isArray(value.tools)) throw new ConnectorProtocolError();
  const tools = value.tools.map((tool: unknown) => {
    if (!isRecord(tool) || typeof tool.modelName !== "string" || typeof tool.rawName !== "string"
      || typeof tool.label !== "string" || typeof tool.description !== "string" || typeof tool.enabled !== "boolean") {
      throw new ConnectorProtocolError();
    }
    return { modelName: tool.modelName, rawName: tool.rawName, label: tool.label, description: tool.description, enabled: tool.enabled };
  });
  return { configKey, generation: value.generation, attempt: value.attempt, tools };
}

export function parseSetConnectorToolEnabledResponse(
  value: unknown,
  expected: Pick<SetConnectorToolEnabledResponse, "configKey" | "rawName" | "enabled">,
): SetConnectorToolEnabledResponse {
  if (!isRecord(value)
    || value.configKey !== expected.configKey
    || value.rawName !== expected.rawName
    || value.enabled !== expected.enabled
    || typeof value.configSaved !== "boolean"
    || typeof value.runtimeApplied !== "boolean") {
    throw new ConnectorProtocolError();
  }
  return {
    configKey: expected.configKey,
    rawName: expected.rawName,
    enabled: expected.enabled,
    configSaved: value.configSaved,
    runtimeApplied: value.runtimeApplied,
  };
}

function isConnectorState(value: unknown): value is ConnectorState {
  return value === "pending" || value === "connecting" || value === "connected"
    || value === "disconnected" || value === "awaiting_project_trust"
    || value === "needs_authorization" || value === "failed";
}

export function normalizeConnectorView(value: unknown): ConnectorView | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const state = raw.state;
  const source = raw.source === "workspace" ? "workspace" : raw.source === "global" ? "global" : null;
  const transport = typeof raw.url === "string" ? "http" : "stdio";
  if (typeof raw.name !== "string" || typeof raw.configKey !== "string" || !source || !isConnectorState(state)) return null;
  const validIdentity = isConnectorGeneration(raw.generation) && isInteger(raw.attempt, 0, 3);
  const validRecovery = raw.recovery === null || isRecoveryProgress(raw.recovery);
  return {
    configKey: raw.configKey,
    name: raw.name,
    type: "mcp",
    transport,
    source,
    overridden: raw.overridden === true,
    auth: raw.auth === "none" || raw.auth === "bearer" || raw.auth === "oauth" ? raw.auth : null,
    oauthConfigured: raw.oauthConfigured === true || raw.auth === "oauth",
    state,
    generation: isConnectorGeneration(raw.generation) ? raw.generation : undefined,
    attempt: isInteger(raw.attempt, 0, 3) ? raw.attempt : undefined,
    recovery: isRecoveryProgress(raw.recovery) ? raw.recovery : null,
    compatibilityError: validIdentity && validRecovery ? undefined : t("connector.protocolMismatch"),
    toolCount: typeof raw.toolCount === "number" ? raw.toolCount : 0,
    resourceCount: typeof raw.resourceCount === "number" ? raw.resourceCount : 0,
    url: typeof raw.url === "string" ? raw.url : null,
    command: typeof raw.command === "string" ? raw.command : null,
    configPath: typeof raw.configPath === "string" ? raw.configPath : null,
    configPathRaw: typeof raw.configPathRaw === "string" ? raw.configPathRaw : null,
    error: typeof raw.error === "string" ? raw.error : null,
    toolFilter: isRecord(raw.toolFilter) ? {
      include: Array.isArray(raw.toolFilter.include) ? raw.toolFilter.include.filter((value): value is string => typeof value === "string") : [],
      exclude: Array.isArray(raw.toolFilter.exclude) ? raw.toolFilter.exclude.filter((value): value is string => typeof value === "string") : [],
    } : undefined,
  };
}
