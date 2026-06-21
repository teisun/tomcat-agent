import type {
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type {
  SessionListPayload,
  SessionStatePayload,
  SessionSummary,
} from "../../serveClient/sessionRouter";
import type { ServeEvent } from "../../serveClient/wire";
import {
  normalizePlanState,
  planEventState,
} from "../participant/planState";
import type {
  FrontendOwnerKind,
  HostEventFrameContent,
  TomcatUiMode,
  WebviewApprovalCard,
  WebviewMessageBlock,
  WebviewSessionSnapshot,
  WebviewSessionTab,
  WebviewStateSnapshot,
  WebviewToolCard,
} from "./protocol";

function cloneSnapshot(snapshot: WebviewStateSnapshot): WebviewStateSnapshot {
  return JSON.parse(JSON.stringify(snapshot)) as WebviewStateSnapshot;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function asText(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }
  if (value === null || value === undefined) {
    return undefined;
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function parseAskQuestionRequest(frame: ControlRequestFrame): AskQuestionWireRequest | null {
  if (!isRecord(frame.payload)) {
    return null;
  }
  const payload = frame.payload;
  if (
    typeof payload.requestId !== "string" ||
    typeof payload.responseEvent !== "string" ||
    !Array.isArray(payload.questions)
  ) {
    return null;
  }
  return payload as unknown as AskQuestionWireRequest;
}

function createEmptySession(sessionId: string): WebviewSessionSnapshot {
  return {
    approvals: [],
    busy: false,
    conflictMessage: null,
    messages: [],
    model: null,
    ownedByThisFrontend: false,
    owner: null,
    planId: null,
    planState: "chat",
    sessionId,
    tools: [],
  };
}

function getAssistantDelta(
  event: ServeEvent,
): { delta: string; kind: string } | null {
  if (event.type !== "message_update" || !isRecord(event.assistantMessageEvent)) {
    return null;
  }
  const delta = event.assistantMessageEvent.delta;
  const kind = event.assistantMessageEvent.kind;
  if (typeof delta !== "string" || typeof kind !== "string") {
    return null;
  }
  return { delta, kind };
}

function ensureMessage(
  session: WebviewSessionSnapshot,
  kind: WebviewMessageBlock["kind"],
): WebviewMessageBlock {
  const current = session.messages.at(-1);
  if (current && current.kind === kind) {
    return current;
  }
  const next: WebviewMessageBlock = {
    id: `${session.sessionId}-${kind}-${session.messages.length + 1}`,
    kind,
    text: "",
  };
  session.messages.push(next);
  return next;
}

function upsertTool(
  session: WebviewSessionSnapshot,
  toolCallId: string,
  toolName: string,
): WebviewToolCard {
  const existing = session.tools.find((tool) => tool.toolCallId === toolCallId);
  if (existing) {
    return existing;
  }
  const next: WebviewToolCard = {
    isError: false,
    status: "running",
    toolCallId,
    toolName,
  };
  session.tools.push(next);
  return next;
}

function mapSessionToTab(
  session: SessionSummary,
  owner: FrontendOwnerKind | null,
  ownedByThisFrontend: boolean,
): WebviewSessionTab {
  return {
    busy: session.busy,
    isCurrent: session.isCurrent,
    ownedByThisFrontend,
    owner,
    sessionId: session.sessionId,
    updatedAt: session.updatedAt,
  };
}

export class WebviewStateStore {
  private state: WebviewStateSnapshot;

  constructor(uiMode: TomcatUiMode = "both") {
    this.state = {
      activeSessionId: null,
      availableModels: [],
      ready: false,
      sessionViews: {},
      sessions: [],
      uiMode,
    };
  }

  snapshot(): WebviewStateSnapshot {
    return cloneSnapshot(this.state);
  }

  setReady(ready: boolean): void {
    this.state.ready = ready;
  }

  setAvailableModels(models: string[]): void {
    this.state.availableModels = [...models];
  }

  setUiMode(mode: TomcatUiMode): void {
    this.state.uiMode = mode;
  }

  setActiveSession(sessionId: string | null): void {
    this.state.activeSessionId = sessionId;
    if (sessionId) {
      this.ensureSession(sessionId);
    }
  }

  syncSessionList(
    payload: SessionListPayload,
    ownership: Map<string, FrontendOwnerKind>,
    frontend: FrontendOwnerKind,
  ): void {
    this.state.sessions = payload.sessions.map((session) =>
      mapSessionToTab(
        session,
        ownership.get(session.sessionId) ?? null,
        ownership.get(session.sessionId) === frontend,
      ),
    );
    if (payload.activeSessionId) {
      this.setActiveSession(payload.activeSessionId);
    }
  }

  applySessionState(
    payload: SessionStatePayload,
    owner: FrontendOwnerKind | null,
    frontend: FrontendOwnerKind,
  ): void {
    const session = this.ensureSession(payload.sessionId);
    session.busy = payload.busy;
    session.model = payload.model ?? null;
    session.planId = payload.planId ?? null;
    session.planState = normalizePlanState(payload.planState) ?? "chat";
    session.owner = owner;
    session.ownedByThisFrontend = owner === frontend;
    this.syncTabOwnership(payload.sessionId, owner, frontend);
  }

  setConflict(sessionId: string, message: string | null): void {
    this.ensureSession(sessionId).conflictMessage = message;
  }

  appendMessage(
    sessionId: string,
    kind: WebviewMessageBlock["kind"],
    text: string,
  ): void {
    ensureMessage(this.ensureSession(sessionId), kind).text += text;
  }

  setOwnership(
    sessionId: string,
    owner: FrontendOwnerKind | null,
    frontend: FrontendOwnerKind,
  ): void {
    const session = this.ensureSession(sessionId);
    session.owner = owner;
    session.ownedByThisFrontend = owner === frontend;
    if (owner !== frontend) {
      session.conflictMessage = null;
    }
    this.syncTabOwnership(sessionId, owner, frontend);
  }

  resolveApproval(requestId: string): void {
    for (const session of Object.values(this.state.sessionViews)) {
      for (const approval of session.approvals) {
        if (approval.request.requestId === requestId) {
          approval.resolved = true;
        }
      }
    }
  }

  applyEvent(frame: HostEventFrameContent): void {
    if (frame.type === "__test.capture_dom") {
      return;
    }
    if ("subtype" in frame && frame.type === "control_request") {
      this.applyControlRequest(frame);
      return;
    }

    const session = this.ensureSession(
      frame.sessionId ?? this.state.activeSessionId ?? "unknown",
    );
    switch (frame.type) {
      case "agent_start":
        session.busy = true;
        return;
      case "agent_end":
        session.busy = false;
        if (frame.error) {
          const message = ensureMessage(session, "error");
          message.text += frame.error;
        }
        return;
      case "agent_interrupted":
        session.busy = false;
        ensureMessage(session, "notice").text += "Tomcat turn interrupted";
        return;
      case "llm_notice":
        ensureMessage(session, "notice").text += frame.message;
        return;
      case "llm_error":
        ensureMessage(
          session,
          "error",
        ).text += `${frame.reason}: ${frame.errorMessage}`;
        return;
      case "message_update": {
        const delta = getAssistantDelta(frame);
        if (!delta) {
          return;
        }
        if (delta.kind === "content_delta") {
          ensureMessage(session, "assistant").text += delta.delta;
          return;
        }
        if (delta.kind === "thinking_delta") {
          ensureMessage(session, "thinking").text += delta.delta;
        }
        return;
      }
      case "tool_execution_start": {
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.status = "running";
        tool.isError = false;
        return;
      }
      case "tool_call_streaming":
      case "tool_execution_update": {
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.status = "streaming";
        return;
      }
      case "tool_execution_end": {
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.display = frame.display ?? undefined;
        tool.isError = frame.isError;
        tool.status = "complete";
        tool.summary = asText(frame.result);
        return;
      }
      case "plan.create":
      case "plan.update":
      case "plan.build":
      case "plan.review":
      case "plan.code_review":
      case "plan.verify":
      case "plan.review.warning":
      case "plan.code_review.warning":
      case "plan.complete": {
        const state = planEventState(frame);
        if (state) {
          session.planState = state;
        }
        if (frame.planId) {
          session.planId = frame.planId;
        }
        return;
      }
      default:
        return;
    }
  }

  private applyControlRequest(frame: ControlRequestFrame): void {
    if (frame.subtype !== "ask_question") {
      return;
    }
    const request = parseAskQuestionRequest(frame);
    if (!request) {
      return;
    }
    const session = this.ensureSession(frame.sessionId ?? this.state.activeSessionId ?? "unknown");
    const approval: WebviewApprovalCard = {
      request,
      resolved: false,
      sessionId: frame.sessionId,
    };
    session.approvals = session.approvals.filter(
      (entry) => entry.request.requestId !== request.requestId,
    );
    session.approvals.push(approval);
  }

  private ensureSession(sessionId: string): WebviewSessionSnapshot {
    const existing = this.state.sessionViews[sessionId];
    if (existing) {
      return existing;
    }
    const created = createEmptySession(sessionId);
    this.state.sessionViews[sessionId] = created;
    return created;
  }

  private syncTabOwnership(
    sessionId: string,
    owner: FrontendOwnerKind | null,
    frontend: FrontendOwnerKind,
  ): void {
    const existing = this.state.sessions.find((session) => session.sessionId === sessionId);
    if (existing) {
      existing.owner = owner;
      existing.ownedByThisFrontend = owner === frontend;
      return;
    }
    this.state.sessions.push({
      busy: false,
      isCurrent: false,
      ownedByThisFrontend: owner === frontend,
      owner,
      sessionId,
      updatedAt: null,
    });
  }
}
