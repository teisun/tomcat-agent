import * as vscode from "vscode";

import { PARTICIPANT_ID } from "../constants";
import type { TomcatMessenger } from "./TomcatMessenger";
import type { ResponseFrame } from "./wire";

export interface SessionSummary {
  busy: boolean;
  sessionId: string;
}

export interface SessionListPayload {
  activeSessionId: string | null;
  sessions: SessionSummary[];
}

export interface SessionStatePayload {
  busy: boolean;
  cwd?: string | null;
  mode?: string | null;
  model?: string | null;
  sessionId: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
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

  async listSessions(): Promise<SessionListPayload> {
    const response = await this.messenger.request({
      type: "list_sessions",
    });
    const payload = response.payload;

    if (!isRecord(payload)) {
      return {
        activeSessionId: null,
        sessions: [],
      };
    }

    return {
      activeSessionId:
        typeof payload.activeSessionId === "string" ? payload.activeSessionId : null,
      sessions: Array.isArray(payload.sessions)
        ? payload.sessions
            .filter(isRecord)
            .map((session) => ({
              busy: session.busy === true,
              sessionId: String(session.sessionId ?? ""),
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
      cwd: typeof payload.cwd === "string" ? payload.cwd : null,
      mode: typeof payload.mode === "string" ? payload.mode : null,
      model: typeof payload.model === "string" ? payload.model : null,
      sessionId: payload.sessionId,
    };
  }
}
