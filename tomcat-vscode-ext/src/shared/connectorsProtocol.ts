export type ConnectorType = "mcp" | "cli" | "a2a";
export type ConnectorTransport = "stdio" | "http";
export type ConnectorState =
  | "pending"
  | "connecting"
  | "connected"
  | "disconnected"
  | "needs_confirmation"
  | "needs_authorization"
  | "blocked"
  | "failed";
export type ConnectorSource = "Global" | "Workspace" | "Unknown";

export interface ConnectorConfigPath {
  display: string;
  raw: string;
}

export interface ConnectorConfigPaths {
  global: ConnectorConfigPath;
  /** Undefined when this session has no explicit project root. */
  workspace?: ConnectorConfigPath;
}

export interface ConnectorView {
  name: string;
  type: ConnectorType;
  transport: ConnectorTransport;
  source: ConnectorSource;
  auth?: "none" | "bearer" | "oauth" | null;
  oauthConfigured: boolean;
  state: ConnectorState;
  trust: string;
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
  scope: "workspace" | "user";
}

export interface ConnectorsHostFrame {
  connectors: ConnectorView[];
  selected?: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeConnectorView(value: unknown): ConnectorView | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const state = typeof raw.state === "string" ? raw.state : "failed";
  const source = raw.source === "Workspace"
    ? "Workspace"
    : raw.source === "Global" || raw.source === "User"
      ? "Global"
      : "Unknown";
  const transport = typeof raw.url === "string" ? "http" : "stdio";
  if (typeof raw.name !== "string") return null;
  return {
    name: raw.name,
    type: "mcp",
    transport,
    source,
    auth: raw.auth === "none" || raw.auth === "bearer" || raw.auth === "oauth" ? raw.auth : null,
    oauthConfigured: raw.oauthConfigured === true || raw.auth === "oauth",
    state: state as ConnectorState,
    trust: typeof raw.trust === "string" ? raw.trust : "unknown",
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
