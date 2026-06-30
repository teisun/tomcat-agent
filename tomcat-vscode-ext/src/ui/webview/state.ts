import type {
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type {
  SessionHistoryPayload,
  SessionListPayload,
  SessionStatePayload,
  SessionSummary,
} from "../../serveClient/sessionRouter";
import { isRecord, parseTodos } from "../../shared/todos";
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
  WebviewBoundaryBlock,
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
  streamingAssistantId: string | null;
  hasMoreHistory: boolean;
  historyEntries: unknown[];
  historyLoading: boolean;
  oldestHistoryCursor: string | null;
};

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
    hasMoreHistory: false,
    historyLoading: false,
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

function timelineEntityKey(item: WebviewTimelineItem): string {
  switch (item.type) {
    case "message":
      if (item.kind === "assistant") {
        return `assistant:${item.assistantMessageId ?? item.id}`;
      }
      return `message:${item.id}`;
    case "thinking":
      return `thinking:${item.assistantMessageId ?? item.id}`;
    case "tool":
      return `tool:${item.toolCallId}`;
    case "approval":
      return `approval:${item.request.requestId}`;
    case "boundary":
      return `boundary:${item.id}`;
    case "plan":
      return `plan:${item.path}`;
  }
}

function planEventMessageId(
  eventType: string,
  planId: string | null | undefined,
  detail: string | null | undefined,
): string {
  return `plan-event:${eventType}:${planId ?? "none"}:${detail && detail.length > 0 ? detail : "default"}`;
}

function cloneTimelineItem<T extends WebviewTimelineItem>(item: T): T {
  return JSON.parse(JSON.stringify(item)) as T;
}

function upsertTimelineItem(session: WebviewSessionSnapshot, item: WebviewTimelineItem): void {
  const key = timelineEntityKey(item);
  const existingIndex = session.timeline.findIndex((entry) => timelineEntityKey(entry) === key);
  if (existingIndex >= 0) {
    session.timeline[existingIndex] = cloneTimelineItem(item);
    return;
  }
  session.timeline.push(cloneTimelineItem(item));
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

function extractSummaryTitle(message: Record<string, unknown>): string | undefined {
  if (typeof message.summary_title === "string" && message.summary_title.trim()) {
    return message.summary_title.trim();
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

function applyHistoryPlanCustomEntry(
  session: WebviewSessionSnapshot,
  entry: Record<string, unknown>,
): void {
  const eventName = typeof entry.event === "string" ? entry.event : null;
  if (!eventName?.startsWith("plan.")) {
    return;
  }
  const preferredId = typeof entry.id === "string" ? entry.id : null;
  const planId = typeof entry.plan_id === "string" ? entry.plan_id : null;
  const path = typeof entry.path === "string" ? entry.path : null;
  const state =
    normalizePlanState(entry.state) ??
    planEventState({ type: eventName } as ServePlanEvent);

  switch (eventName) {
    case "plan.create":
    case "plan.build":
    case "plan.update":
    case "plan.complete":
    case "plan.pending":
      if (!path) {
        return;
      }
      upsertPlanFile(session, path, state, planId);
      return;
    case "plan.todos": {
      if (!planId) {
        return;
      }
      const card = session.timeline.find(
        (item): item is WebviewPlanFileCard =>
          item.type === "plan" && item.planId === planId,
      );
      if (card) {
        card.todos = parseTodos(entry.todos);
      }
      return;
    }
    case "plan.review":
    case "plan.code_review":
      if (typeof entry.summary === "string" && entry.summary.length > 0) {
        pushMessage(
          session,
          "notice",
          `Tomcat plan review: ${entry.summary}`,
          planEventMessageId(eventName, planId, entry.summary),
        );
      }
      return;
    case "plan.verify":
      if (typeof entry.verdict === "string" && entry.verdict.length > 0) {
        pushMessage(
          session,
          "notice",
          `Tomcat plan verify: ${entry.verdict}`,
          planEventMessageId(eventName, planId, entry.verdict),
        );
      }
      return;
    case "plan.review.warning":
    case "plan.code_review.warning":
      {
        const reason =
          typeof entry.reason === "string" && entry.reason.length > 0
            ? entry.reason
            : "review needs attention";
      pushMessage(
        session,
        "warn",
        `Tomcat plan warning: ${reason}`,
        planEventMessageId(eventName, planId, reason),
      );
      }
      return;
    default:
      return;
  }
}

function applyHistoryEntry(
  session: WebviewSessionSnapshot,
  entry: unknown,
  historyToolNames: Map<string, string>,
  toolCallToAssistant: Map<string, string>,
  historyToolArgs: Map<string, Record<string, unknown>>,
): void {
  if (!isRecord(entry) || typeof entry.type !== "string") {
    return;
  }

  if (entry.type === "branch_summary") {
    if (entry.isBoundary !== true) {
      return;
    }
    session.timeline.push({
      coveredCount: typeof entry.coveredCount === "number" ? entry.coveredCount : null,
      id: typeof entry.id === "string" ? entry.id : `boundary-${session.timeline.length + 1}`,
      summary: typeof entry.summary === "string" ? entry.summary : null,
      type: "boundary",
    } satisfies WebviewBoundaryBlock);
    return;
  }

  if (entry.type === "message" && isRecord(entry.message)) {
    const role = typeof entry.message.role === "string" ? entry.message.role : null;
    const text = extractMessageText(entry.message.content);
    const id =
      typeof entry.id === "string" ? entry.id : `history-message-${(text ?? role ?? "unknown").length}`;
    if (role === "user") {
      if (!text) {
        return;
      }
      session.timeline.push({
        id,
        kind: "user",
        text,
        type: "message",
      } satisfies WebviewMessageBlock);
      return;
    }
    if (role === "assistant") {
      const hasToolCalls =
        Array.isArray(entry.message.tool_calls) && entry.message.tool_calls.length > 0;
      const thinkingText = extractThinkingText(entry.message);
      const summaryTitle = extractSummaryTitle(entry.message) ?? null;
      const assistantMessageId = id;
      if (hasToolCalls || thinkingText) {
        session.timeline.push({
          assistantMessageId,
          id: `${id}-thinking`,
          summaryTitle,
          text: thinkingText ?? "",
          type: "thinking",
        } satisfies WebviewThinkingBlock);
      }
      if (text) {
        session.timeline.push({
          assistantMessageId,
          id,
          kind: "assistant",
          text,
          type: "message",
        } satisfies WebviewMessageBlock);
      }
      return;
    }
    if (role === "tool") {
      if (!text) {
        return;
      }
      const toolCallId = extractToolCallId(entry.message) ?? id;
      const args = historyToolArgs.get(toolCallId);
      session.timeline.push({
        args,
        assistantMessageId: toolCallToAssistant.get(toolCallId),
        id,
        isError: false,
        status: "complete",
        summary: text,
        toolCallId,
        toolName: historyToolNames.get(toolCallId) ?? "tool",
        type: "tool",
      } satisfies WebviewToolCard);
      return;
    }
  }

  if (entry.type === "thinking_trace" && typeof entry.text === "string" && entry.text.trim()) {
    session.timeline.push({
      id: typeof entry.id === "string" ? entry.id : `thinking-${entry.text.length}`,
      text: entry.text,
      type: "thinking",
    } satisfies WebviewThinkingBlock);
    return;
  }

  if (entry.type === "custom") {
    applyHistoryPlanCustomEntry(session, entry);
  }
}

function mergeCurrentPlanCardsIntoHistory(
  historySession: WebviewSessionSnapshot,
  liveSession: WebviewSessionSnapshot,
): void {
  for (const item of liveSession.timeline) {
    if (item.type !== "plan") {
      continue;
    }
    const card = upsertPlanFile(historySession, item.path, item.state, item.planId ?? null);
    card.title = item.title;
    card.overview = item.overview;
  }
  if (liveSession.planFile?.path) {
    upsertPlanFile(
      historySession,
      liveSession.planFile.path,
      liveSession.planFile.state,
      liveSession.planFile.planId ?? liveSession.planId ?? null,
    );
  }
}

function createSessionRuntime(): SessionRuntimeState {
  return {
    activeAssistantId: null,
    activeThinkingId: null,
    streamingAssistantId: null,
    hasMoreHistory: false,
    historyEntries: [],
    historyLoading: false,
    oldestHistoryCursor: null,
  };
}

function historyEntryKey(entry: unknown): string {
  if (isRecord(entry) && typeof entry.id === "string") {
    return `id:${entry.id}`;
  }
  try {
    return `json:${JSON.stringify(entry)}`;
  } catch {
    return `fallback:${String(entry)}`;
  }
}

function mergeHistoryEntries(older: unknown[], newer: unknown[]): unknown[] {
  const seen = new Set<string>();
  const merged: unknown[] = [];
  for (const entry of [...older, ...newer]) {
    const key = historyEntryKey(entry);
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    merged.push(entry);
  }
  return merged;
}

function isHistoryChildEntry(entry: unknown): boolean {
  if (!isRecord(entry) || typeof entry.type !== "string") {
    return false;
  }
  if (entry.type === "thinking_trace") {
    return true;
  }
  return (
    entry.type === "message" &&
    isRecord(entry.message) &&
    entry.message.role === "tool"
  );
}

function trimLeadingHistoryEntries(entries: unknown[]): unknown[] {
  let start = 0;
  while (start < entries.length && isHistoryChildEntry(entries[start])) {
    start += 1;
  }
  return start === 0 ? entries : entries.slice(start);
}

function clearStreaming(runtime: SessionRuntimeState): void {
  runtime.streamingAssistantId = null;
  runtime.activeThinkingId = null;
}

function clearThinkingStreaming(runtime: SessionRuntimeState): void {
  runtime.activeThinkingId = null;
}

function clearActiveAssistant(runtime: SessionRuntimeState): void {
  runtime.activeAssistantId = null;
  runtime.streamingAssistantId = null;
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

function findThinkingByAssistantMessageId(
  session: WebviewSessionSnapshot,
  assistantMessageId: string,
): WebviewThinkingBlock | undefined {
  return [...session.timeline].reverse().find(
    (item): item is WebviewThinkingBlock =>
      item.type === "thinking" && item.assistantMessageId === assistantMessageId,
  );
}

function ensureThinkingBlockForAssistantMessage(
  session: WebviewSessionSnapshot,
  runtime: SessionRuntimeState,
  assistantMessageId: string,
): WebviewThinkingBlock {
  const current = runtime.activeThinkingId
    ? findTimelineItem(session, runtime.activeThinkingId, "thinking")
    : undefined;
  if (current && current.assistantMessageId === assistantMessageId) {
    return current;
  }

  const existing = findThinkingByAssistantMessageId(session, assistantMessageId);
  if (existing) {
    runtime.activeThinkingId = existing.id;
    return existing;
  }

  const created: WebviewThinkingBlock = {
    assistantMessageId,
    id: `${assistantMessageId}-thinking`,
    summaryTitle: null,
    text: "",
    type: "thinking",
  };
  const assistantIndex = findTimelineIndex(session, assistantMessageId, "message");
  if (assistantIndex >= 0) {
    session.timeline.splice(assistantIndex, 0, created);
  } else {
    session.timeline.push(created);
  }
  runtime.activeThinkingId = created.id;
  return created;
}

function findAssistantGroupIdForToolCallIds(
  session: WebviewSessionSnapshot,
  toolCallIds: string[],
): string | undefined {
  for (const toolCallId of toolCallIds) {
    const tool = session.timeline.find(
      (item): item is WebviewToolCard =>
        item.type === "tool" && item.toolCallId === toolCallId && !!item.assistantMessageId,
    );
    if (tool?.assistantMessageId) {
      return tool.assistantMessageId;
    }
  }
  return undefined;
}

function applySummaryTitleToGroup(
  session: WebviewSessionSnapshot,
  runtime: SessionRuntimeState,
  summaryTitle: string,
  options: {
    assistantMessageId?: string | null;
    toolCallIds?: string[];
  },
): void {
  const assistantMessageId =
    findAssistantGroupIdForToolCallIds(session, options.toolCallIds ?? []) ??
    (typeof options.assistantMessageId === "string" && options.assistantMessageId.length > 0
      ? options.assistantMessageId
      : undefined) ??
    runtime.activeAssistantId ??
    [...session.timeline]
      .reverse()
      .find(
        (item): item is WebviewThinkingBlock =>
          item.type === "thinking" && !!item.assistantMessageId,
      )
      ?.assistantMessageId;
  if (!assistantMessageId) {
    return;
  }
  const thinking = ensureThinkingBlockForAssistantMessage(
    session,
    runtime,
    assistantMessageId,
  );
  thinking.summaryTitle = summaryTitle;
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

function effectiveBusy(busy: boolean, interrupted: boolean | null | undefined): boolean {
  return busy && interrupted !== true;
}

function messageExistsAtTail(
  session: WebviewSessionSnapshot,
  kind: WebviewMessageBlock["kind"],
  text: string,
): boolean {
  const last = session.timeline.at(-1);
  return last?.type === "message" && last.kind === kind && last.text === text;
}

function toolResultWasInterrupted(result: unknown): boolean {
  return typeof result === "string" && result.trim() === "[interrupted]";
}

function markRunningToolsInterrupted(session: WebviewSessionSnapshot): void {
  for (const item of session.timeline) {
    if (item.type !== "tool") {
      continue;
    }
    if (item.status === "running" || item.status === "streaming") {
      item.status = "interrupted";
      item.isError = false;
      item.summary = "Interrupted";
    }
  }
}

function appendStreamingMessage(
  session: WebviewSessionSnapshot,
  runtime: SessionRuntimeState,
  kind: "assistant" | "thinking",
  assistantMessageId: string,
  text: string,
): WebviewMessageBlock | WebviewThinkingBlock {
  if (kind === "assistant") {
    const current = findTimelineItem(session, assistantMessageId, "message");
    if (current && current.kind === "assistant") {
      current.text += text;
      return current;
    }
    const created = pushMessage(session, "assistant", text, assistantMessageId);
    created.assistantMessageId = assistantMessageId;
    runtime.activeAssistantId = assistantMessageId;
    return created;
  }

  const current = ensureThinkingBlockForAssistantMessage(session, runtime, assistantMessageId);
  current.text += text;
  return current;
}

function mapSessionToTab(
  session: SessionSummary,
  owner: FrontendOwnerKind | null,
  ownedByThisFrontend: boolean,
): WebviewSessionTab {
  return {
    busy: effectiveBusy(session.busy, session.interrupted),
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

  resetForReload(): void {
    const uiMode = this.state.uiMode;
    this.runtimes.clear();
    this.state = {
      activeSessionId: null,
      availableModels: [],
      ready: false,
      sessionViews: {},
      sessions: [],
      uiMode,
    };
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
    if (!this.state.activeSessionId && payload.activeSessionId) {
      this.setActiveSession(payload.activeSessionId);
    }
  }

  applySessionState(
    payload: SessionStatePayload,
    owner: FrontendOwnerKind | null,
    frontend: FrontendOwnerKind,
  ): void {
    const session = this.ensureSession(payload.sessionId);
    session.busy = effectiveBusy(payload.busy, payload.interrupted);
    session.model = payload.model ?? null;
    session.thinkingLevel = payload.thinkingLevel ?? null;
    session.planId = payload.planId ?? null;
    session.planState = normalizePlanState(payload.planState) ?? "chat";
    session.planTodos = payload.planTodos ?? session.planTodos;
    session.sessionTodos = payload.sessionTodos ?? session.sessionTodos;
    if (payload.contextRatio !== undefined) {
      session.contextRatio = payload.contextRatio ?? null;
    }
    if (typeof payload.planPath === "string" && payload.planPath.length > 0) {
      syncPlanRef(
        session,
        payload.planPath,
        session.planState ?? null,
        session.planId ?? null,
      );
      upsertPlanFile(
        session,
        payload.planPath,
        session.planState ?? null,
        session.planId ?? null,
      );
    } else if (session.planFile) {
      const nextState = session.planState ?? session.planFile.state ?? null;
      const nextPlanId = session.planId ?? session.planFile.planId ?? null;
      session.planFile = {
        ...session.planFile,
        planId: nextPlanId,
        state: nextState,
      };
      upsertPlanFile(session, session.planFile.path, nextState, nextPlanId);
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
    this.appendLatestHistory(sessionId, history);
  }

  appendLatestHistory(sessionId: string, history: SessionHistoryPayload): void {
    const runtime = this.ensureRuntime(sessionId);
    runtime.historyEntries = Array.isArray(history.messages) ? [...history.messages] : [];
    runtime.oldestHistoryCursor = history.nextCursor ?? null;
    runtime.hasMoreHistory = history.hasMore === true && typeof history.nextCursor === "string";
    runtime.historyLoading = false;
    this.rebuildHistoryTimeline(sessionId);
  }

  prependHistory(sessionId: string, history: SessionHistoryPayload): void {
    this.prependOlderHistory(sessionId, history);
  }

  prependOlderHistory(sessionId: string, history: SessionHistoryPayload): void {
    const runtime = this.ensureRuntime(sessionId);
    runtime.historyEntries = mergeHistoryEntries(history.messages, runtime.historyEntries);
    runtime.oldestHistoryCursor = history.nextCursor ?? null;
    runtime.hasMoreHistory = history.hasMore === true && typeof history.nextCursor === "string";
    runtime.historyLoading = false;
    this.rebuildHistoryTimeline(sessionId);
  }

  setHistoryLoading(sessionId: string, loading: boolean): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    runtime.historyLoading = loading;
    session.historyLoading = loading;
  }

  getOldestHistoryCursor(sessionId: string): string | null {
    return this.ensureRuntime(sessionId).oldestHistoryCursor;
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
    if (owner === null || owner === frontend) {
      session.conflictMessage = null;
    } else if (!session.conflictMessage) {
      session.conflictMessage =
        owner === "participant"
          ? "This session is currently owned by the Tomcat participant."
          : "This session is currently owned by the Tomcat webview.";
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
        clearStreaming(runtime);
        return;
      case "message_start": {
        clearStreaming(runtime);
        if ("assistantMessageId" in frame && typeof frame.assistantMessageId === "string") {
          runtime.activeAssistantId = frame.assistantMessageId;
          runtime.streamingAssistantId = frame.assistantMessageId;
        }
        return;
      }
      case "message_end":
        if (
          !("assistantMessageId" in frame) ||
          typeof frame.assistantMessageId !== "string" ||
          runtime.streamingAssistantId === frame.assistantMessageId
        ) {
          clearStreaming(runtime);
        }
        return;
      case "turn_end": {
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (summaryTitle) {
          applySummaryTitleToGroup(session, runtime, summaryTitle, {
            assistantMessageId:
              "assistantMessageId" in frame && typeof frame.assistantMessageId === "string"
                ? frame.assistantMessageId
                : undefined,
            toolCallIds:
              "toolCallIds" in frame && Array.isArray(frame.toolCallIds)
                ? frame.toolCallIds.filter(
                    (toolCallId): toolCallId is string => typeof toolCallId === "string",
                  )
                : [],
          });
        }
        clearActiveAssistant(runtime);
        return;
      }
      case "agent_start":
        session.busy = true;
        clearActiveAssistant(runtime);
        return;
      case "agent_end":
        session.busy = false;
        clearActiveAssistant(runtime);
        if (frame.error && frame.error !== "interrupted") {
          pushMessage(session, "error", frame.error);
        }
        return;
      case "agent_interrupted":
        session.busy = false;
        clearActiveAssistant(runtime);
        markRunningToolsInterrupted(session);
        if (!messageExistsAtTail(session, "warn", "Tomcat turn interrupted")) {
          pushMessage(session, "warn", "Tomcat turn interrupted");
        }
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
        const assistantMessageId =
          "assistantMessageId" in frame && typeof frame.assistantMessageId === "string"
            ? frame.assistantMessageId
            : null;
        if (!assistantMessageId) {
          return;
        }
        if (!runtime.streamingAssistantId && !runtime.activeAssistantId) {
          runtime.activeAssistantId = assistantMessageId;
          runtime.streamingAssistantId = assistantMessageId;
        }
        if (runtime.streamingAssistantId !== assistantMessageId) {
          return;
        }
        if (delta.kind === "content_delta") {
          runtime.activeAssistantId = assistantMessageId;
          appendStreamingMessage(session, runtime, "assistant", assistantMessageId, delta.delta);
          return;
        }
        if (delta.kind === "thinking_delta") {
          runtime.activeAssistantId = assistantMessageId;
          appendStreamingMessage(session, runtime, "thinking", assistantMessageId, delta.delta);
        }
        return;
      }
      case "tool_execution_start": {
        clearThinkingStreaming(runtime);
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.status = "running";
        tool.isError = false;
        tool.args = parseToolArgs(frame.args) ?? tool.args;
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        return;
      }
      case "tool_call_streaming":
      case "tool_execution_update": {
        clearThinkingStreaming(runtime);
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
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
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.display = frame.display ?? undefined;
        tool.isError = frame.isError;
        tool.status = toolResultWasInterrupted(frame.result) ? "interrupted" : "complete";
        tool.summary = toolResultWasInterrupted(frame.result) ? "Interrupted" : asText(frame.result);
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        return;
      }
      case "plan.todos": {
        const todos = parseTodos("todos" in frame ? frame.todos : undefined);
        session.planTodos = todos;
        const planId =
          "planId" in frame && typeof frame.planId === "string" ? frame.planId : null;
        if (planId) {
          const card = session.timeline.find(
            (item): item is WebviewPlanFileCard =>
              item.type === "plan" && item.planId === planId,
          );
          if (card) {
            card.todos = todos;
          }
        }
        return;
      }
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
      case "turn.summary_updated": {
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (!summaryTitle) {
          return;
        }
        applySummaryTitleToGroup(session, runtime, summaryTitle, {
          assistantMessageId:
            "assistantMessageId" in frame && typeof frame.assistantMessageId === "string"
              ? frame.assistantMessageId
              : undefined,
          toolCallIds:
            "toolCallIds" in frame && Array.isArray(frame.toolCallIds)
              ? frame.toolCallIds.filter(
                  (toolCallId): toolCallId is string => typeof toolCallId === "string",
                )
              : [],
        });
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

  private rebuildHistoryTimeline(sessionId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const renderableEntries = trimLeadingHistoryEntries(runtime.historyEntries);
    const historyToolNames = buildHistoryToolNameLookup(renderableEntries);
    const toolCallToAssistant = buildToolCallToAssistantMap(renderableEntries);
    const historyToolArgs = buildHistoryToolArgsLookup(renderableEntries);
    const historySession = createEmptySession(sessionId);
    for (const entry of renderableEntries) {
      applyHistoryEntry(
        historySession,
        entry,
        historyToolNames,
        toolCallToAssistant,
        historyToolArgs,
      );
    }
    mergeCurrentPlanCardsIntoHistory(historySession, session);
    const existingKeys = new Set(historySession.timeline.map((item) => timelineEntityKey(item)));
    for (const item of session.timeline) {
      if (item.type === "plan") {
        continue;
      }
      const key = timelineEntityKey(item);
      if (existingKeys.has(key)) {
        continue;
      }
      upsertTimelineItem(historySession, item);
      existingKeys.add(key);
    }
    session.timeline = historySession.timeline;
    if (session.planTodos.length === 0 && session.planId) {
      const activeCard = session.timeline.find(
        (item): item is WebviewPlanFileCard =>
          item.type === "plan" &&
          item.planId === session.planId &&
          Array.isArray(item.todos) &&
          item.todos.length > 0,
      );
      if (activeCard?.todos) {
        session.planTodos = activeCard.todos;
      }
    }
    session.hasMoreHistory = runtime.hasMoreHistory;
    session.historyLoading = runtime.historyLoading;
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
          pushMessage(
            session,
            "notice",
            `Tomcat plan review: ${event.summary}`,
            planEventMessageId(event.type, event.planId, event.summary),
          );
        }
        return;
      case "plan.verify":
        if (event.verdict) {
          pushMessage(
            session,
            "notice",
            `Tomcat plan verify: ${event.verdict}`,
            planEventMessageId(event.type, event.planId, event.verdict),
          );
        }
        return;
      case "plan.review.warning":
      case "plan.code_review.warning":
        {
          const reason = event.reason ?? "review needs attention";
        pushMessage(
          session,
          "warn",
          `Tomcat plan warning: ${reason}`,
          planEventMessageId(event.type, event.planId, reason),
        );
        }
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
