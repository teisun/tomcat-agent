import * as vscode from "vscode";

import { PARTICIPANT_ID } from "../constants";
import { isRecord, parseTodos } from "../shared/todos";
import type { TomcatMessenger } from "./TomcatMessenger";
import type { GetMessagesParams, ListSessionsScope, ResponseFrame } from "./wire";

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
  busy: boolean;
  contextRatio?: number | null;
  cwd?: string | null;
  interrupted?: boolean;
  mode?: string | null;
  model?: string | null;
  planId?: string | null;
  planPath?: string | null;
  planState?: string | null;
  planTodos?: WebviewTodo[];
  sessionId: string;
  sessionKey?: string | null;
  sessionTodos?: WebviewTodo[];
  thinkingLevel?: string | null;
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

function readSessionIdFromHistoryTurn(turn: unknown): string | undefined {
  if (!isRecord(turn) || turn.participant !== PARTICIPANT_ID || !isRecord(turn.result)) {
    return undefined;
  }

  const metadata = turn.result.metadata;
  return isRecord(metadata) && typeof metadata.sessionId === "string"
    ? metadata.sessionId
    : undefined;
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

  buildResultMetadata(sessionId: string): vscode.ChatResult["metadata"] {
    return { sessionId };
  }

  extractSessionId(
    history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
  ): string | undefined {
    for (let index = history.length - 1; index >= 0; index -= 1) {
      const sessionId = readSessionIdFromHistoryTurn(history[index]);
      if (sessionId) {
        return sessionId;
      }
    }

    return undefined;
  }

  async resolveSessionId(
    history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
  ): Promise<string> {
    const historySessionId = this.extractSessionId(history);
    if (historySessionId) {
      return historySessionId;
    }

    const bootstrapSessionId = this.takeBootstrapSessionId();
    if (bootstrapSessionId) {
      return bootstrapSessionId;
    }

    return this.newSession();
  }

  async newSession(cwd = this.getDefaultCwd()): Promise<string> {
    const response = await this.messenger.request({
      params: {
        cwd,
      },
      type: "new_session",
    });
    return requireSessionId(response);
  }

  async switchSession(sessionId: string): Promise<string> {
    const response = await this.messenger.request({
      sessionId,
      type: "switch_session",
    });
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
      mode: typeof payload.mode === "string" ? payload.mode : null,
      model: typeof payload.model === "string" ? payload.model : null,
      planId: typeof payload.planId === "string" ? payload.planId : null,
      planPath: typeof payload.planPath === "string" ? payload.planPath : null,
      planState:
        typeof payload.planState === "string" ? payload.planState : null,
      planTodos: parseTodos(payload.planTodos),
      sessionId: payload.sessionId,
      sessionKey:
        typeof payload.sessionKey === "string" ? payload.sessionKey : null,
      sessionTodos: parseTodos(payload.sessionTodos),
      thinkingLevel:
        typeof payload.thinkingLevel === "string" ? payload.thinkingLevel : null,
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
}
