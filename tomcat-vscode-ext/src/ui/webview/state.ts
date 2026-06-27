import type {
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type {
  SessionHistoryPayload,
  SessionListPayload,
  SessionStatePayload,
  SessionSummary,
  WebviewTodo,
} from "../../serveClient/sessionRouter";
import type { ServeEvent, ServePlanEvent } from "../../serveClient/wire";
import {
  normalizePlanState,
  planEventState,
  type ParticipantPlanState,
} from "../participant/planState";
import type {
  FrontendOwnerKind,
  HostEventFrameContent,
  TomcatUiMode,
  WebviewApprovalCard,
  WebviewMessageBlock,
  WebviewPendingAttachment,
  WebviewPlanFileCard,
  WebviewPlanFileRef,
  WebviewSessionSnapshot,
  WebviewSessionTab,
  WebviewStateSnapshot,
  WebviewThinkingBlock,
  WebviewTimelineItem,
  WebviewToolCard,
} from "./protocol";

function cloneSnapshot(snapshot: WebviewStateSnapshot): WebviewStateSnapshot {
  return JSON.parse(JSON.stringify(snapshot)) as WebviewStateSnapshot;
}

type SessionRuntimeState = {
  activeAssistantId: string | null;
  activeThinkingId: string | null;
  historyHydrated: boolean;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isPlanEvent(event: ServeEvent): event is ServePlanEvent {
  return event.type.startsWith("plan.");
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
    busy: false,
    conflictMessage: null,
    contextRatio: null,
    model: null,
    planTodos: [],
    sessionTodos: [],
    thinkingLevel: null,
    ownedByThisFrontend: false,
    owner: null,
    pendingAttachments: [],
    planFile: null,
    planId: null,
    planState: "chat",
    sessionId,
    timeline: [],
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

function createTimelineId(
  session: WebviewSessionSnapshot,
  prefix: string,
  preferredId?: string | null,
): string {
  if (preferredId) {
    return preferredId;
  }
  return `${session.sessionId}-${prefix}-${session.timeline.length + 1}`;
}

function timelineMergeKeys(item: WebviewTimelineItem): string[] {
  switch (item.type) {
    case "message":
      return [`message:id:${item.id}`, `message:text:${item.kind}:${item.text}`];
    case "thinking":
      return [`thinking:id:${item.id}`, `thinking:text:${item.text}`];
    case "tool":
      return [`tool:${item.toolCallId}`];
    case "approval":
      return [`approval:${item.request.requestId}`];
    case "plan":
      return [`plan:${item.path}:${item.planId ?? ""}:${item.state ?? "unknown"}`];
  }
}

function extractMessageText(content: unknown): string | undefined {
  if (typeof content === "string") {
    return content;
  }
  if (Array.isArray(content)) {
    const parts = content
      .map((entry) => {
        if (typeof entry === "string") {
          return entry;
        }
        if (!isRecord(entry)) {
          return undefined;
        }
        if (typeof entry.text === "string") {
          return entry.text;
        }
        switch (entry.type) {
          case "input_image":
          case "image":
            return "[image attachment]";
          case "input_file":
          case "file":
            return "[file attachment]";
          default:
            return undefined;
        }
      })
      .filter((entry): entry is string => Boolean(entry));
    return parts.length ? parts.join("\n") : undefined;
  }
  if (isRecord(content) && typeof content.text === "string") {
    return content.text;
  }
  return asText(content);
}

function extractThinkingText(message: Record<string, unknown>): string | undefined {
  if (typeof message.thinking_text === "string" && message.thinking_text.trim()) {
    return message.thinking_text;
  }
  if (
    isRecord(message.reasoning_continuation) &&
    typeof message.reasoning_continuation.fallback_text === "string" &&
    message.reasoning_continuation.fallback_text.trim()
  ) {
    return message.reasoning_continuation.fallback_text;
  }
  return undefined;
}

function extractToolCallId(message: Record<string, unknown>): string | undefined {
  return typeof message.tool_call_id === "string" ? message.tool_call_id : undefined;
}

function buildHistoryToolNameLookup(entries: unknown[]): Map<string, string> {
  const lookup = new Map<string, string>();
  for (const entry of entries) {
    if (
      !isRecord(entry) ||
      entry.type !== "message" ||
      !isRecord(entry.message) ||
      entry.message.role !== "assistant" ||
      !Array.isArray(entry.message.tool_calls)
    ) {
      continue;
    }
    for (const toolCall of entry.message.tool_calls) {
      if (
        !isRecord(toolCall) ||
        typeof toolCall.id !== "string" ||
        !isRecord(toolCall.function) ||
        typeof toolCall.function.name !== "string"
      ) {
        continue;
      }
      lookup.set(toolCall.id, toolCall.function.name);
    }
  }
  return lookup;
}

export function buildToolCallToAssistantMap(entries: unknown[]): Map<string, string> {
  const lookup = new Map<string, string>();
  for (const entry of entries) {
    if (
      !isRecord(entry) ||
      entry.type !== "message" ||
      !isRecord(entry.message) ||
      entry.message.role !== "assistant" ||
      !Array.isArray(entry.message.tool_calls)
    ) {
      continue;
    }
    const assistantId = typeof entry.id === "string" ? entry.id : undefined;
    if (!assistantId) {
      continue;
    }
    for (const toolCall of entry.message.tool_calls) {
      if (!isRecord(toolCall) || typeof toolCall.id !== "string") {
        continue;
      }
      lookup.set(toolCall.id, assistantId);
    }
  }
  return lookup;
}

function buildHistoryToolArgsLookup(entries: unknown[]): Map<string, Record<string, unknown>> {
  const lookup = new Map<string, Record<string, unknown>>();
  for (const entry of entries) {
    if (
      !isRecord(entry) ||
      entry.type !== "message" ||
      !isRecord(entry.message) ||
      entry.message.role !== "assistant" ||
      !Array.isArray(entry.message.tool_calls)
    ) {
      continue;
    }
    for (const toolCall of entry.message.tool_calls) {
      if (
        !isRecord(toolCall) ||
        typeof toolCall.id !== "string" ||
        !isRecord(toolCall.function)
      ) {
        continue;
      }
      const rawArgs = toolCall.function.arguments;
      if (typeof rawArgs === "string") {
        try {
          const parsed = JSON.parse(rawArgs) as unknown;
          if (isRecord(parsed)) {
            lookup.set(toolCall.id, parsed);
          }
        } catch {
          // ignore malformed tool arguments
        }
      } else if (isRecord(rawArgs)) {
        lookup.set(toolCall.id, rawArgs);
      }
    }
  }
  return lookup;
}

function parseTodoStatus(value: unknown): WebviewTodo["status"] | null {
  switch (value) {
    case "pending":
    case "in_progress":
    case "completed":
    case "cancelled":
      return value;
    default:
      return null;
  }
}

function parseTodos(value: unknown): WebviewTodo[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    if (!isRecord(entry) || typeof entry.id !== "string" || typeof entry.content !== "string") {
      return [];
    }
    const status = parseTodoStatus(entry.status);
    if (!status) {
      return [];
    }
    return [{ content: entry.content, id: entry.id, status }];
  });
}

function parseToolArgs(value: unknown): Record<string, unknown> | undefined {
  if (isRecord(value)) {
    return value;
  }
  if (typeof value === "string") {
    try {
      const parsed = JSON.parse(value) as unknown;
      return isRecord(parsed) ? parsed : undefined;
    } catch {
      return undefined;
    }
  }
  return undefined;
}

function parseHistoryEntry(
  entry: unknown,
  historyToolNames: Map<string, string>,
  toolCallToAssistant: Map<string, string>,
  historyToolArgs: Map<string, Record<string, unknown>>,
): WebviewTimelineItem[] {
  if (!isRecord(entry) || typeof entry.type !== "string") {
    return [];
  }

  if (entry.type === "message" && isRecord(entry.message)) {
    const role = typeof entry.message.role === "string" ? entry.message.role : null;
    const text = extractMessageText(entry.message.content);
    const id =
      typeof entry.id === "string" ? entry.id : `history-message-${(text ?? role ?? "unknown").length}`;
    if (role === "user") {
      if (!text) {
        return [];
      }
      return [
        {
          id,
          kind: "user",
          text,
          type: "message",
        } satisfies WebviewMessageBlock,
      ];
    }
    if (role === "assistant") {
      const items: WebviewTimelineItem[] = [];
      const hasToolCalls =
        Array.isArray(entry.message.tool_calls) && entry.message.tool_calls.length > 0;
      const assistantMessageId = hasToolCalls ? id : undefined;
      const thinkingText = extractThinkingText(entry.message);
      if (thinkingText) {
        items.push({
          assistantMessageId,
          id: `${id}-thinking`,
          summaryTitle: null,
          text: thinkingText,
          type: "thinking",
        } satisfies WebviewThinkingBlock);
      }
      if (text) {
        items.push({
          assistantMessageId,
          id,
          kind: "assistant",
          text,
          type: "message",
        } satisfies WebviewMessageBlock);
      }
      return items;
    }
    if (role === "tool") {
      if (!text) {
        return [];
      }
      const toolCallId = extractToolCallId(entry.message) ?? id;
      const args = historyToolArgs.get(toolCallId);
      return [{
        args,
        assistantMessageId: toolCallToAssistant.get(toolCallId),
        id,
        isError: false,
        status: "complete",
        summary: text,
        toolCallId,
        toolName: historyToolNames.get(toolCallId) ?? "tool",
        type: "tool",
      } satisfies WebviewToolCard];
    }
  }

  if (entry.type === "thinking_trace" && typeof entry.text === "string" && entry.text.trim()) {
    return [{
      id: typeof entry.id === "string" ? entry.id : `thinking-${entry.text.length}`,
      text: entry.text,
      type: "thinking",
    } satisfies WebviewThinkingBlock];
  }

  return [];
}

function createSessionRuntime(): SessionRuntimeState {
  return {
    activeAssistantId: null,
    activeThinkingId: null,
    historyHydrated: false,
  };
}

function clearStreaming(runtime: SessionRuntimeState): void {
  runtime.activeAssistantId = null;
  runtime.activeThinkingId = null;
}

function clearThinkingStreaming(runtime: SessionRuntimeState): void {
  runtime.activeThinkingId = null;
}

function upsertPlanFile(
  session: WebviewSessionSnapshot,
  path: string,
  state: ParticipantPlanState | null,
  planId?: string | null,
): WebviewPlanFileCard {
  const existing = session.timeline.find(
    (item): item is WebviewPlanFileCard => item.type === "plan" && item.path === path,
  );
  if (existing) {
    existing.planId = planId ?? existing.planId ?? null;
    existing.state = state ?? existing.state ?? null;
    return existing;
  }
  const created: WebviewPlanFileCard = {
    id: createTimelineId(session, "plan", path),
    path,
    planId: planId ?? null,
    state,
    type: "plan",
  };
  session.timeline.push(created);
  return created;
}

function syncPlanRef(
  session: WebviewSessionSnapshot,
  path: string,
  state: ParticipantPlanState | null,
  planId?: string | null,
): void {
  session.planFile = {
    path,
    planId: planId ?? session.planId ?? null,
    state,
  } satisfies WebviewPlanFileRef;
}

function findTimelineItem<T extends WebviewTimelineItem["type"]>(
  session: WebviewSessionSnapshot,
  id: string,
  type: T,
): Extract<WebviewTimelineItem, { type: T }> | undefined {
  return session.timeline.find(
    (item): item is Extract<WebviewTimelineItem, { type: T }> => item.id === id && item.type === type,
  );
}

function findTimelineIndex<T extends WebviewTimelineItem["type"]>(
  session: WebviewSessionSnapshot,
  id: string,
  type: T,
): number {
  return session.timeline.findIndex((item) => item.id === id && item.type === type);
}

function upsertTool(
  session: WebviewSessionSnapshot,
  toolCallId: string,
  toolName: string,
): WebviewToolCard {
  const existing = session.timeline.find(
    (item): item is WebviewToolCard => item.type === "tool" && item.toolCallId === toolCallId,
  );
  if (existing) {
    return existing;
  }
  const next: WebviewToolCard = {
    id: createTimelineId(session, "tool", toolCallId),
    isError: false,
    status: "running",
    toolCallId,
    toolName,
    type: "tool",
  };
  session.timeline.push(next);
  return next;
}

function upsertApproval(
  session: WebviewSessionSnapshot,
  request: AskQuestionWireRequest,
  sessionId?: string | null,
): WebviewApprovalCard {
  const existing = session.timeline.find(
    (item): item is WebviewApprovalCard =>
      item.type === "approval" && item.request.requestId === request.requestId,
  );
  if (existing) {
    existing.request = request;
    existing.resolved = false;
    existing.sessionId = sessionId;
    return existing;
  }
  const created: WebviewApprovalCard = {
    id: createTimelineId(session, "approval", request.requestId),
    request,
    resolved: false,
    sessionId,
    type: "approval",
  };
  session.timeline.push(created);
  return created;
}

function pushMessage(
  session: WebviewSessionSnapshot,
  kind: WebviewMessageBlock["kind"],
  text: string,
  preferredId?: string | null,
): WebviewMessageBlock {
  const next: WebviewMessageBlock = {
    id: createTimelineId(session, kind, preferredId),
    kind,
    text,
    type: "message",
  };
  session.timeline.push(next);
  return next;
}

function appendStreamingMessage(
  session: WebviewSessionSnapshot,
  runtime: SessionRuntimeState,
  kind: "assistant" | "thinking",
  text: string,
): WebviewMessageBlock | WebviewThinkingBlock {
  if (kind === "assistant") {
    const current = runtime.activeAssistantId
      ? findTimelineItem(session, runtime.activeAssistantId, "message")
      : undefined;
    if (current && current.kind === "assistant") {
      current.text += text;
      return current;
    }
    const created = pushMessage(session, "assistant", text);
    created.assistantMessageId = created.id;
    runtime.activeAssistantId = created.id;
    return created;
  }

  const current = runtime.activeThinkingId
    ? findTimelineItem(session, runtime.activeThinkingId, "thinking")
    : undefined;
  if (current) {
    current.text += text;
    return current;
  }
  const created: WebviewThinkingBlock = {
    assistantMessageId: runtime.activeAssistantId ?? undefined,
    id: createTimelineId(session, "thinking"),
    summaryTitle: null,
    text,
    type: "thinking",
  };
  const assistantIndex = runtime.activeAssistantId
    ? findTimelineIndex(session, runtime.activeAssistantId, "message")
    : -1;
  if (assistantIndex >= 0) {
    session.timeline.splice(assistantIndex, 0, created);
  } else {
    session.timeline.push(created);
  }
  runtime.activeThinkingId = created.id;
  return created;
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
    title: session.title,
    updatedAt: session.updatedAt,
  };
}

export class WebviewStateStore {
  private state: WebviewStateSnapshot;
  private readonly runtimes = new Map<string, SessionRuntimeState>();

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
    session.thinkingLevel = payload.thinkingLevel ?? null;
    session.planId = payload.planId ?? null;
    session.planState = normalizePlanState(payload.planState) ?? "chat";
    session.planTodos = payload.planTodos ?? session.planTodos;
    session.sessionTodos = payload.sessionTodos ?? session.sessionTodos;
    if (session.planFile) {
      session.planFile = {
        ...session.planFile,
        planId: session.planId ?? session.planFile.planId ?? null,
        state: session.planState ?? session.planFile.state ?? null,
      };
    }
    session.owner = owner;
    session.ownedByThisFrontend = owner === frontend;
    this.syncTabOwnership(payload.sessionId, owner, frontend);
  }

  setConflict(sessionId: string, message: string | null): void {
    this.ensureSession(sessionId).conflictMessage = message;
  }

  setPendingAttachments(sessionId: string, attachments: WebviewPendingAttachment[]): void {
    this.ensureSession(sessionId).pendingAttachments = [...attachments];
  }

  clearPendingAttachments(sessionId: string): void {
    this.ensureSession(sessionId).pendingAttachments = [];
  }

  removePendingAttachment(sessionId: string, attachmentId: string): void {
    const session = this.ensureSession(sessionId);
    session.pendingAttachments = session.pendingAttachments.filter(
      (attachment) => attachment.id !== attachmentId,
    );
  }

  hydrateHistory(sessionId: string, history: SessionHistoryPayload): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const historyToolNames = buildHistoryToolNameLookup(history.messages);
    const toolCallToAssistant = buildToolCallToAssistantMap(history.messages);
    const historyToolArgs = buildHistoryToolArgsLookup(history.messages);
    const historyItems = history.messages.flatMap((entry) =>
      parseHistoryEntry(entry, historyToolNames, toolCallToAssistant, historyToolArgs),
    );
    if (!historyItems.length) {
      runtime.historyHydrated = true;
      return;
    }

    const existingKeys = new Set(historyItems.flatMap((item) => timelineMergeKeys(item)));
    const liveOnly = session.timeline.filter(
      (item) => !timelineMergeKeys(item).some((key) => existingKeys.has(key)),
    );
    session.timeline = [...historyItems, ...liveOnly];
    runtime.historyHydrated = true;
  }

  appendMessage(
    sessionId: string,
    kind: WebviewMessageBlock["kind"],
    text: string,
  ): void {
    if (!text) {
      return;
    }
    pushMessage(this.ensureSession(sessionId), kind, text);
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
      for (const item of session.timeline) {
        if (item.type === "approval" && item.request.requestId === requestId) {
          item.resolved = true;
        }
      }
    }
  }

  applyEvent(frame: HostEventFrameContent): void {
    if (frame.type === "__test.capture_dom" || frame.type === "__test.dom_action") {
      return;
    }
    if ("subtype" in frame && frame.type === "control_request") {
      this.applyControlRequest(frame);
      return;
    }

    const session = this.ensureSession(
      frame.sessionId ?? this.state.activeSessionId ?? "unknown",
    );
    const runtime = this.ensureRuntime(session.sessionId);
    switch (frame.type) {
      case "turn_start":
      case "message_start":
        clearStreaming(runtime);
        return;
      case "message_end":
        clearStreaming(runtime);
        return;
      case "turn_end": {
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (summaryTitle) {
          const thinking =
            (runtime.activeThinkingId
              ? findTimelineItem(session, runtime.activeThinkingId, "thinking")
              : undefined) ??
            [...session.timeline].reverse().find(
              (item): item is WebviewThinkingBlock => item.type === "thinking",
            );
          if (thinking) {
            thinking.summaryTitle = summaryTitle;
          }
        }
        clearStreaming(runtime);
        return;
      }
      case "agent_start":
        session.busy = true;
        clearStreaming(runtime);
        return;
      case "agent_end":
        session.busy = false;
        clearStreaming(runtime);
        if (frame.error) {
          pushMessage(session, "error", frame.error);
        }
        return;
      case "agent_interrupted":
        session.busy = false;
        clearStreaming(runtime);
        pushMessage(session, "notice", "Tomcat turn interrupted");
        return;
      case "llm_notice":
        pushMessage(session, "notice", frame.message);
        return;
      case "llm_error":
        pushMessage(session, "error", `${frame.reason}: ${frame.errorMessage}`);
        return;
      case "extension_error":
        pushMessage(session, "error", `${frame.event}: ${frame.error}`);
        return;
      case "context_metrics_update":
        session.contextRatio = frame.contextUtilizationRatio;
        return;
      case "compaction_error":
        pushMessage(session, "notice", `Context compaction failed: ${frame.error}`);
        return;
      case "auto_retry_start":
        pushMessage(session, "notice", `Retrying after error: ${frame.errorMessage}`);
        return;
      case "auto_retry_end":
        if (!frame.success) {
          pushMessage(session, "notice", `Retry finished without success: ${frame.finalError ?? "unknown error"}`);
        }
        return;
      case "sub_agent_start":
        pushMessage(session, "notice", `Started ${frame.subagentType} sub-agent`);
        return;
      case "sub_agent_end":
        pushMessage(session, "notice", `Sub-agent ${frame.subagentType} ${frame.outcome}`);
        return;
      case "message_update": {
        const delta = getAssistantDelta(frame);
        if (!delta) {
          return;
        }
        if (delta.kind === "content_delta") {
          appendStreamingMessage(session, runtime, "assistant", delta.delta);
          return;
        }
        if (delta.kind === "thinking_delta") {
          appendStreamingMessage(session, runtime, "thinking", delta.delta);
        }
        return;
      }
      case "tool_execution_start": {
        const activeAssistantId = runtime.activeAssistantId;
        clearThinkingStreaming(runtime);
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.status = "running";
        tool.isError = false;
        tool.args = parseToolArgs(frame.args) ?? tool.args;
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        return;
      }
      case "tool_call_streaming":
      case "tool_execution_update": {
        const activeAssistantId = runtime.activeAssistantId;
        clearThinkingStreaming(runtime);
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.status = "streaming";
        if ("args" in frame) {
          tool.args = parseToolArgs(frame.args) ?? tool.args;
        }
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        return;
      }
      case "tool_execution_end": {
        clearThinkingStreaming(runtime);
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.display = frame.display ?? undefined;
        tool.isError = frame.isError;
        tool.status = "complete";
        tool.summary = asText(frame.result);
        return;
      }
      case "plan.todos":
        session.planTodos = parseTodos("todos" in frame ? frame.todos : undefined);
        return;
      case "session.todos":
        session.sessionTodos = parseTodos("todos" in frame ? frame.todos : undefined);
        return;
      case "session.title_updated": {
        const title =
          "title" in frame && typeof frame.title === "string" ? frame.title : null;
        if (!title) {
          return;
        }
        const tab = this.state.sessions.find(
          (entry) => entry.sessionId === session.sessionId,
        );
        if (tab) {
          tab.title = title;
        }
        return;
      }
      default:
        if (isPlanEvent(frame)) {
          this.applyPlanEvent(session, frame);
        }
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
    upsertApproval(session, request, frame.sessionId);
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

  private ensureRuntime(sessionId: string): SessionRuntimeState {
    const existing = this.runtimes.get(sessionId);
    if (existing) {
      return existing;
    }
    const created = createSessionRuntime();
    this.runtimes.set(sessionId, created);
    return created;
  }

  private applyPlanEvent(
    session: WebviewSessionSnapshot,
    event: ServePlanEvent,
  ): void {
    const state = planEventState(event);
    if (state) {
      session.planState = state;
    }
    if (event.planId) {
      session.planId = event.planId;
    }
    if ("path" in event && typeof event.path === "string" && event.path.length > 0) {
      const nextState = state ?? session.planState ?? null;
      syncPlanRef(session, event.path, nextState, event.planId ?? session.planId ?? null);
      upsertPlanFile(session, event.path, nextState, event.planId ?? session.planId ?? null);
    } else if (session.planFile) {
      session.planFile = {
        ...session.planFile,
        planId: event.planId ?? session.planFile.planId ?? null,
        state: state ?? session.planFile.state ?? null,
      };
    }

    switch (event.type) {
      case "plan.review":
      case "plan.code_review":
        if (event.summary) {
          pushMessage(session, "notice", `Tomcat plan review: ${event.summary}`);
        }
        return;
      case "plan.verify":
        if (event.verdict) {
          pushMessage(session, "notice", `Tomcat plan verify: ${event.verdict}`);
        }
        return;
      case "plan.review.warning":
      case "plan.code_review.warning":
        pushMessage(
          session,
          "notice",
          `Tomcat plan warning: ${event.reason ?? "review needs attention"}`,
        );
        return;
      default:
        return;
    }
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
      title: null,
      updatedAt: null,
    });
  }
}
