import { isRecord, parseTodos } from "../shared/todos";
import type { TomcatMessenger } from "./TomcatMessenger";
import type {
  GetMessagesParams,
  ListSessionsScope,
  ResponseFrame,
  SlashReply,
  RetainAttachmentLeaseRef,
  SessionFile,
  SessionFilesResponse,
  SessionFileBaselineResponse,
  SessionFilesRestoreResponse,
} from "./wire";

export interface SessionSummary {
  busy: boolean;
  interrupted?: boolean;
  isCurrent: boolean;
  sessionId: string;
  title: string | null;
  updatedAt: number | null;
}

export interface SessionListPayload {
  activeSessionId: string | null;
  scope: ListSessionsScope;
  sessions: SessionSummary[];
}

export interface SessionStatePayload {
  activePlan?: ActivePlanPayload | null;
  agentMode?: "chat" | "plan" | null;
  busy: boolean;
  contextRatio?: number | null;
  cwd?: string | null;
  interrupted?: boolean;
  model?: string | null;
  planTodos?: WebviewTodo[];
  sessionId: string;
  sessionKey?: string | null;
  sessionTodos?: WebviewTodo[];
  thinkingLevel?: string | null;
  workspaceMode?: string | null;
}

export interface ActivePlanPayload {
  id: string;
  path: string;
  state: "completed" | "executing" | "pending" | "planning";
}

export interface WebviewTodo {
  content: string;
  id: string;
  status: "cancelled" | "completed" | "in_progress" | "pending";
}

export interface SessionHistoryPayload {
  hasMore?: boolean;
  header?: unknown;
  messages: unknown[];
  nextCursor?: string | null;
  sessionId: string;
  upToSeq?: string | null;
}

export interface SessionCheckpointPayload {
  changedFiles: string[];
  createdAt: string;
  id: string;
  kind: string;
  label?: string | null;
  messageAnchor?: string | null;
}

export interface SessionCheckpointListPayload {
  checkpoints: SessionCheckpointPayload[];
  sessionId: string;
}

export interface RestoreCheckpointPayload {
  changedPaths: string[];
  checkpointId: string;
  createdAt: string;
  dryRun: boolean;
  kind: string;
  label?: string | null;
  messageAnchor?: string | null;
  reloadedPlanId?: string | null;
  restoredPaths: string[];
  revertFiles: boolean;
  sessionId: string;
  summary?: string | null;
  transcriptTruncated: boolean;
  warnings: string[];
}

export interface CompactPayload {
  afterUsageRatio: number;
  beforeUsageRatio: number;
  coveredMessageCount: number;
}

function requireSuccessfulResponse(response: ResponseFrame, action: string): void {
  if (!response.success) {
    throw new Error(response.error ?? `Tomcat ${action} failed`);
  }
}

function requireSessionId(response: ResponseFrame): string {
  if (typeof response.sessionId === "string") {
    return response.sessionId;
  }

  if (isRecord(response.payload) && typeof response.payload.sessionId === "string") {
    return response.payload.sessionId;
  }

  throw new Error("Tomcat response did not include a sessionId");
}

function parseStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string")
    : [];
}

function parseCheckpoints(value: unknown): SessionCheckpointPayload[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    if (!isRecord(entry) || typeof entry.id !== "string") {
      return [];
    }
    return [{
      changedFiles: parseStringArray(entry.changedFiles),
      createdAt: typeof entry.createdAt === "string" ? entry.createdAt : "",
      id: entry.id,
      kind: typeof entry.kind === "string" ? entry.kind : "",
      label:
        entry.label === undefined || entry.label === null || typeof entry.label === "string"
          ? entry.label
          : null,
      messageAnchor:
        entry.messageAnchor === undefined ||
        entry.messageAnchor === null ||
        typeof entry.messageAnchor === "string"
          ? entry.messageAnchor
          : null,
    }];
  });
}

function parseActivePlan(value: unknown): ActivePlanPayload | null {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.path !== "string") {
    return null;
  }
  switch (value.state) {
    case "completed":
    case "executing":
    case "pending":
    case "planning":
      return { id: value.id, path: value.path, state: value.state };
    default:
      return null;
  }
}

export class SessionRouter {
  private bootstrapSessionId: string | null = null;

  constructor(
    private readonly messenger: TomcatMessenger,
    private readonly getDefaultCwd: () => string | undefined,
  ) {}

  setBootstrapSessionId(sessionId: string | null): void {
    this.bootstrapSessionId = sessionId;
  }

  takeBootstrapSessionId(): string | null {
    const value = this.bootstrapSessionId;
    this.bootstrapSessionId = null;
    return value;
  }

  clearBootstrapSessionId(): void {
    this.bootstrapSessionId = null;
  }

  async newSession(cwd = this.getDefaultCwd()): Promise<string> {
    const response = await this.messenger.request({
      params: {
        cwd,
      },
      type: "new_session",
    });
    requireSuccessfulResponse(response, "new_session");
    return requireSessionId(response);
  }

  async createDetachedSession(cwd = this.getDefaultCwd()): Promise<string> {
    const response = await this.messenger.request({
      params: {
        cwd,
        detached: true,
      },
      type: "new_session",
    });
    requireSuccessfulResponse(response, "detached new_session");
    return requireSessionId(response);
  }

  async retainAttachmentLeases(
    sessionId: string,
    attachments: RetainAttachmentLeaseRef[],
  ): Promise<string[]> {
    const response = await this.messenger.request({
      params: { attachments },
      sessionId,
      type: "retain_attachment_leases",
    });
    requireSuccessfulResponse(response, "retain_attachment_leases");
    const retained = isRecord(response.payload) ? response.payload.retainedShas : null;
    if (!Array.isArray(retained) || !retained.every((sha): sha is string => typeof sha === "string")) {
      throw new Error("Tomcat retain_attachment_leases response was invalid");
    }
    return retained;
  }

  async discardDetachedSession(sessionId: string): Promise<boolean> {
    const response = await this.messenger.request({
      sessionId,
      type: "discard_detached_session",
    });
    requireSuccessfulResponse(response, "discard_detached_session");
    return isRecord(response.payload) ? response.payload.discarded === true : false;
  }

  async switchSession(sessionId: string): Promise<string> {
    const response = await this.messenger.request({
      sessionId,
      type: "switch_session",
    });
    requireSuccessfulResponse(response, "switch_session");
    return requireSessionId(response);
  }

  async closeSession(sessionId: string): Promise<boolean> {
    const response = await this.messenger.request({
      sessionId,
      type: "close_session",
    });
    return isRecord(response.payload) ? response.payload.closed === true : response.success;
  }

  async listSessions(scope: ListSessionsScope = "live"): Promise<SessionListPayload> {
    const response = await this.messenger.request({
      scope,
      type: "list_sessions",
    });
    const payload = response.payload;

    if (!isRecord(payload)) {
      return {
        activeSessionId: null,
        scope,
        sessions: [],
      };
    }

    return {
      activeSessionId:
        typeof payload.activeSessionId === "string" ? payload.activeSessionId : null,
      scope,
      sessions: Array.isArray(payload.sessions)
        ? payload.sessions
            .filter(isRecord)
            .map((session) => ({
              busy: session.busy === true,
              interrupted: session.interrupted === true,
              isCurrent: session.isCurrent === true,
              sessionId: String(session.sessionId ?? ""),
              title:
                typeof session.title === "string" && session.title.length > 0
                  ? session.title
                  : null,
              updatedAt:
                typeof session.updatedAt === "number" ? session.updatedAt : null,
            }))
            .filter((session) => session.sessionId.length > 0)
        : [],
    };
  }

  async getState(sessionId?: string): Promise<SessionStatePayload> {
    const response = await this.messenger.request({
      sessionId,
      type: "get_state",
    });
    const payload = response.payload;

    if (!isRecord(payload) || typeof payload.sessionId !== "string") {
      throw new Error("Tomcat get_state payload is missing sessionId");
    }

    return {
      busy: payload.busy === true,
      contextRatio:
        typeof payload.contextUtilizationRatio === "number"
          ? payload.contextUtilizationRatio
          : null,
      cwd: typeof payload.cwd === "string" ? payload.cwd : null,
      interrupted: payload.interrupted === true,
      activePlan: parseActivePlan(payload.activePlan),
      agentMode:
        payload.agentMode === "chat" || payload.agentMode === "plan"
          ? payload.agentMode
          : null,
      model: typeof payload.model === "string" ? payload.model : null,
      planTodos: parseTodos(payload.planTodos),
      sessionId: payload.sessionId,
      sessionKey:
        typeof payload.sessionKey === "string" ? payload.sessionKey : null,
      sessionTodos: parseTodos(payload.sessionTodos),
      thinkingLevel:
        typeof payload.thinkingLevel === "string" ? payload.thinkingLevel : null,
      workspaceMode:
        typeof payload.workspaceMode === "string" ? payload.workspaceMode : null,
    };
  }

  async getMessages(
    sessionId?: string,
    params: GetMessagesParams = {},
  ): Promise<SessionHistoryPayload> {
    const response = await this.messenger.sendGetMessages(sessionId, params);
    const payload = response.payload;

    if (!isRecord(payload) || typeof payload.sessionId !== "string") {
      throw new Error("Tomcat get_messages payload is missing sessionId");
    }

    return {
      hasMore: payload.hasMore === true,
      header: payload.header,
      messages: Array.isArray(payload.messages) ? payload.messages : [],
      nextCursor: typeof payload.nextCursor === "string" ? payload.nextCursor : null,
      sessionId: payload.sessionId,
      upToSeq: typeof payload.upToSeq === "string" ? payload.upToSeq : null,
    };
  }

  private filePayload(response: ResponseFrame, sessionId: string): Record<string, unknown> {
    if (!response.success) {
      const payload = isRecord(response.payload) ? response.payload : {};
      throw new Error([response.error ?? "File operation failed", payload.path, payload.reason].filter(Boolean).join(": "));
    }
    if (!isRecord(response.payload) || response.payload.sessionId !== sessionId || response.sessionId !== sessionId) {
      throw new Error("Tomcat file response sessionId mismatch");
    }
    return response.payload;
  }

  async getSessionFiles(sessionId: string): Promise<SessionFilesResponse> {
    const payload = this.filePayload(await this.messenger.request({ type: "get_session_files", sessionId }), sessionId);
    if (!(payload.sourceTurnId === null || typeof payload.sourceTurnId === "string") || !Array.isArray(payload.files)) {
      throw new Error("Invalid Tomcat file changes response");
    }
    const files: SessionFile[] = payload.files.map((entry: unknown) => {
      if (!isRecord(entry) || typeof entry.path !== "string" || typeof entry.restorable !== "boolean"
        || !["added", "modified", "deleted"].includes(String(entry.status))
        || (entry.blockedReason !== undefined && !["head_moved", "backup_missing", "not_regular_file"].includes(String(entry.blockedReason)))
        || [entry.added, entry.removed].some((n) => n !== undefined && n !== null && (typeof n !== "number" || !Number.isInteger(n) || n < 0))) {
        throw new Error("Invalid Tomcat file changes entry");
      }
      return entry as unknown as SessionFile;
    });
    return { sessionId, sourceTurnId: payload.sourceTurnId, files };
  }

  async getSessionFileBaseline(sessionId: string, sourceTurnId: string, path: string): Promise<SessionFileBaselineResponse> {
    const payload = this.filePayload(await this.messenger.request({ type: "get_session_file_baseline", sessionId, sourceTurnId, path }), sessionId);
    if (payload.sourceTurnId !== sourceTurnId || payload.path !== path || typeof payload.existed !== "boolean" || typeof payload.text !== "string") {
      throw new Error("Invalid Tomcat file baseline response");
    }
    return { sessionId, sourceTurnId, path, existed: payload.existed, text: payload.text };
  }

  async restoreSessionFiles(sessionId: string, sourceTurnId: string, paths: string[]): Promise<SessionFilesRestoreResponse> {
    const payload = this.filePayload(await this.messenger.request({ type: "restore_session_files", sessionId, sourceTurnId, paths }), sessionId);
    if (payload.sourceTurnId !== sourceTurnId || !Array.isArray(payload.restored) || !payload.restored.every((p) => typeof p === "string" && paths.includes(p))) {
      throw new Error("Invalid Tomcat file restore response");
    }
    return { sessionId, sourceTurnId, restored: payload.restored as string[] };
  }

  async listCheckpoints(sessionId: string): Promise<SessionCheckpointListPayload> {
    const response = await this.messenger.request({
      sessionId,
      type: "list_checkpoints",
    } as never);
    if (!response.success) {
      throw new Error(response.error ?? "Tomcat list_checkpoints failed");
    }
    const payload = response.payload;

    if (!isRecord(payload) || typeof payload.sessionId !== "string") {
      throw new Error("Tomcat list_checkpoints payload is missing sessionId");
    }

    return {
      checkpoints: parseCheckpoints(payload.checkpoints),
      sessionId: payload.sessionId,
    };
  }

  async restoreCheckpoint(
    sessionId: string,
    checkpointId: string,
    revertFiles: boolean,
    dryRun?: boolean,
  ): Promise<RestoreCheckpointPayload> {
    const response = await this.messenger.request({
      checkpointId,
      dryRun,
      revertFiles,
      sessionId,
      type: "restore_checkpoint",
    } as never);
    if (!response.success) {
      throw new Error(response.error ?? "Tomcat restore_checkpoint failed");
    }
    const payload = response.payload;

    if (
      !isRecord(payload) ||
      typeof payload.sessionId !== "string" ||
      typeof payload.checkpointId !== "string"
    ) {
      throw new Error("Tomcat restore_checkpoint payload is missing identifiers");
    }

    return {
      changedPaths: parseStringArray(payload.changedPaths),
      checkpointId: payload.checkpointId,
      createdAt: typeof payload.createdAt === "string" ? payload.createdAt : "",
      dryRun: payload.dryRun === true,
      kind: typeof payload.kind === "string" ? payload.kind : "",
      label:
        payload.label === undefined || payload.label === null || typeof payload.label === "string"
          ? payload.label
          : null,
      messageAnchor:
        payload.messageAnchor === undefined ||
        payload.messageAnchor === null ||
        typeof payload.messageAnchor === "string"
          ? payload.messageAnchor
          : null,
      reloadedPlanId:
        payload.reloadedPlanId === undefined ||
        payload.reloadedPlanId === null ||
        typeof payload.reloadedPlanId === "string"
          ? payload.reloadedPlanId
          : null,
      restoredPaths: parseStringArray(payload.restoredPaths),
      revertFiles: payload.revertFiles === true,
      sessionId: payload.sessionId,
      summary:
        payload.summary === undefined || payload.summary === null || typeof payload.summary === "string"
          ? payload.summary
          : null,
      transcriptTruncated: payload.transcriptTruncated === true,
      warnings: parseStringArray(payload.warnings),
    };
  }

  async runSlashCommand(sessionId: string, text: string): Promise<SlashReply> {
    const response = await this.messenger.request({ sessionId, text, type: "run_slash_command" }, 600_000);
    requireSuccessfulResponse(response, "run_slash_command");
    const payload = response.payload;
    if (!isRecord(payload) || typeof payload.ok !== "boolean" || typeof payload.text !== "string") {
      throw new Error("Tomcat run_slash_command payload is invalid");
    }
    return { ok: payload.ok, text: payload.text };
  }

  async compact(sessionId: string): Promise<CompactPayload> {
    const response = await this.messenger.request({
      sessionId,
      type: "compact",
    } as never);
    if (!response.success) {
      throw new Error(response.error ?? "Tomcat compact failed");
    }
    const payload = response.payload;
    if (
      !isRecord(payload) ||
      typeof payload.beforeUsageRatio !== "number" ||
      typeof payload.afterUsageRatio !== "number" ||
      typeof payload.coveredMessageCount !== "number"
    ) {
      throw new Error("Tomcat compact payload is invalid");
    }
    return {
      afterUsageRatio: payload.afterUsageRatio,
      beforeUsageRatio: payload.beforeUsageRatio,
      coveredMessageCount: payload.coveredMessageCount,
    };
  }

  async resume(sessionId: string): Promise<void> {
    const response = await this.messenger.request({
      sessionId,
      type: "resume",
    } as never);
    if (!response.success) {
      throw new Error(response.error ?? "Tomcat resume failed");
    }
  }

  async retry(sessionId: string, messageId: string): Promise<void> {
    const response = await this.messenger.request({
      messageId,
      sessionId,
      type: "retry",
    } as never);
    if (!response.success) {
      throw new Error(response.error ?? "Tomcat retry failed");
    }
  }
}
