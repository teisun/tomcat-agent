import type {
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type {
  SessionCheckpointPayload,
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
} from "../../shared/planState";
import type {
  HostEventFrameContent,
  WebviewApprovalCard,
  WebviewBoundaryBlock,
  WebviewMessageBlock,
  WebviewMessageSegment,
  WebviewPendingAttachment,
  WebviewPlanFileRef,
  WebviewReviewFinding,
  WebviewReviewRow,
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
  localUserMessageIds: Set<string>;
  oldestHistoryCursor: string | null;
};

type UserSubmitKind = "prompt" | "steer";

type AppendMessageOptions = {
  detailText?: string | null;
  deliveryError?: string | null;
  deliveryState?: "failed" | "pending";
  preferredId?: string | null;
  retryable?: boolean;
  segments?: WebviewMessageSegment[];
  submitKind?: UserSubmitKind;
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
    checkpoints: [],
    contextRatio: null,
    hasMoreHistory: false,
    historyLoading: false,
    model: null,
    planTodos: [],
    sessionTodos: [],
    thinkingLevel: null,
    ownedByThisFrontend: true,
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
    case "checkpoint":
      return `checkpoint:${item.checkpointId}`;
    case "plan":
      return `plan:${item.planId ?? item.path}`;
    case "review":
      return `review:${item.planId}`;
  }
}

function isSupersededMessageEntry(entry: unknown): boolean {
  return (
    isRecord(entry) &&
    entry.type === "message" &&
    isRecord(entry.message) &&
    entry.message.superseded === true
  );
}

function isCheckpointRestoreEntry(entry: unknown): boolean {
  return (
    isRecord(entry) &&
    entry.type === "custom" &&
    (
      entry.customType === "checkpoint.restore" ||
      (isRecord(entry.extra) && entry.extra.customType === "checkpoint.restore")
    )
  );
}

function isTurnFailedMessageEntry(entry: unknown): boolean {
  return (
    isRecord(entry) &&
    entry.type === "message" &&
    isRecord(entry.message) &&
    entry.message.turn_failed === true
  );
}

function isVisibleUserMessageEntry(entry: unknown): boolean {
  return (
    isRecord(entry) &&
    entry.type === "message" &&
    isRecord(entry.message) &&
    entry.message.role === "user" &&
    entry.message.superseded !== true
  );
}

function filterSupersededHistoryEntries(entries: unknown[]): unknown[] {
  const filtered: unknown[] = [];
  let inSupersededSpan = false;
  for (const entry of entries) {
    if (isSupersededMessageEntry(entry)) {
      if (isTurnFailedMessageEntry(entry)) {
        filtered.push(entry);
        continue;
      }
      inSupersededSpan = true;
      continue;
    }
    if (inSupersededSpan) {
      if (isCheckpointRestoreEntry(entry)) {
        inSupersededSpan = false;
        continue;
      }
      if (isVisibleUserMessageEntry(entry)) {
        inSupersededSpan = false;
        filtered.push(entry);
      }
      continue;
    }
    filtered.push(entry);
  }
  return filtered;
}

function planEventMessageId(
  eventType: string,
  planId: string | null | undefined,
  detail: string | null | undefined,
): string {
  return `plan-event:${eventType}:${planId ?? "none"}:${detail && detail.length > 0 ? detail : "default"}`;
}

function activePlanId(session: WebviewSessionSnapshot): string | null {
  return session.planId ?? session.planFile?.planId ?? null;
}

function parseReviewVerdict(value: unknown): WebviewReviewRow["verdict"] | undefined {
  return value === "pass" || value === "fail" || value === "partial" || value === "aborted"
    ? value
    : undefined;
}

function parseReviewFindings(value: unknown): WebviewReviewFinding[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    if (!isRecord(entry)) {
      return [];
    }
    const severity = typeof entry.severity === "string" ? entry.severity : "";
    const area = typeof entry.area === "string" ? entry.area : "";
    const note = typeof entry.note === "string" ? entry.note : "";
    if (!note) {
      return [];
    }
    return [{ severity, area, note } satisfies WebviewReviewFinding];
  });
}

function upsertRunningCodeReviewRow(session: WebviewSessionSnapshot, planId: string): void {
  upsertTimelineItem(session, {
    id: `review:${planId}`,
    planId,
    status: "running",
    type: "review",
  } satisfies WebviewReviewRow);
}

function upsertDoneCodeReviewRow(
  session: WebviewSessionSnapshot,
  input: {
    aborted?: unknown;
    findings?: unknown;
    planId: string;
    rounds?: unknown;
    summary?: unknown;
    verdict?: unknown;
  },
): void {
  const verdict =
    parseReviewVerdict(input.verdict) ?? (input.aborted === true ? "aborted" : undefined);
  upsertTimelineItem(session, {
    findings: parseReviewFindings(input.findings),
    id: `review:${input.planId}`,
    planId: input.planId,
    rounds: typeof input.rounds === "number" ? input.rounds : null,
    status: "done",
    summary: typeof input.summary === "string" ? input.summary : null,
    type: "review",
    verdict,
  } satisfies WebviewReviewRow);
}

function settleRunningCodeReviewAsAborted(
  session: WebviewSessionSnapshot,
  planId: string,
): void {
  const existing = session.timeline.find(
    (item): item is WebviewReviewRow => item.type === "review" && item.planId === planId,
  );
  if (!existing || existing.status !== "running") {
    return;
  }
  upsertTimelineItem(session, {
    ...existing,
    status: "done",
    summary:
      existing.summary ?? "Code review ended before a structured verdict was emitted.",
    verdict: "aborted",
  });
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

function pushTextSegment(segments: WebviewMessageSegment[], text: string): void {
  if (!text) {
    return;
  }
  const last = segments.at(-1);
  if (last?.type === "text") {
    last.text += text;
    return;
  }
  segments.push({
    text,
    type: "text",
  });
}

function contentToMessageSegments(content: unknown): WebviewMessageSegment[] | undefined {
  if (typeof content === "string") {
    return content ? [{ text: content, type: "text" }] : undefined;
  }
  if (Array.isArray(content)) {
    const segments: WebviewMessageSegment[] = [];
    for (const entry of content) {
      if (typeof entry === "string") {
        pushTextSegment(segments, entry);
        continue;
      }
      if (!isRecord(entry)) {
        continue;
      }
      switch (entry.type) {
        case "input_text":
        case "text":
          if (typeof entry.text === "string") {
            pushTextSegment(segments, entry.text);
          }
          break;
        case "input_reference":
          if (
            (entry.ref_kind === "selection" || entry.ref_kind === "file") &&
            typeof entry.path === "string" &&
            typeof entry.label === "string"
          ) {
            segments.push({
              kind: entry.ref_kind,
              label: entry.label,
              lineEnd: typeof entry.line_end === "number" ? entry.line_end : null,
              lineStart: typeof entry.line_start === "number" ? entry.line_start : null,
              path: entry.path,
              text: typeof entry.text === "string" ? entry.text : null,
              type: "reference",
            });
          }
          break;
        case "input_image":
        case "image":
          pushTextSegment(segments, "[image attachment]");
          break;
        case "input_file":
        case "file":
          pushTextSegment(segments, "[file attachment]");
          break;
        default:
          if (typeof entry.text === "string") {
            pushTextSegment(segments, entry.text);
          }
          break;
      }
    }
    return segments.length ? segments : undefined;
  }
  if (isRecord(content)) {
    if (
      content.type === "input_reference" &&
      (content.ref_kind === "selection" || content.ref_kind === "file") &&
      typeof content.path === "string" &&
      typeof content.label === "string"
    ) {
      return [
        {
          kind: content.ref_kind,
          label: content.label,
          lineEnd: typeof content.line_end === "number" ? content.line_end : null,
          lineStart: typeof content.line_start === "number" ? content.line_start : null,
          path: content.path,
          text: typeof content.text === "string" ? content.text : null,
          type: "reference",
        },
      ];
    }
    if (typeof content.text === "string") {
      return [{ text: content.text, type: "text" }];
    }
  }
  return undefined;
}

function extractMessageText(content: unknown): string | undefined {
  const segments = contentToMessageSegments(content);
  if (segments?.length) {
    const text = segments
      .map((segment) => (segment.type === "text" ? segment.text : segment.label))
      .join("");
    return text || undefined;
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

function asNonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function asFiniteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function parseToolSummaryJson(resultText: string | undefined): Record<string, unknown> | undefined {
  if (!resultText) {
    return undefined;
  }
  try {
    const parsed = JSON.parse(resultText) as unknown;
    return isRecord(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function applyBackgroundTaskTicket(tool: WebviewToolCard, resultText?: string): void {
  const runsInBackground =
    tool.args?.run_in_background === true || tool.args?.runInBackground === true;
  if (
    !runsInBackground ||
    (tool.toolName !== "bash" && tool.toolName !== "shell" && tool.toolName !== "execute_command")
  ) {
    delete tool.backgroundTaskId;
    delete tool.backgroundRunning;
    delete tool.backgroundExitCode;
    return;
  }
  const parsed = parseToolSummaryJson(resultText);
  const taskId = asNonEmptyString(parsed?.taskId) ?? asNonEmptyString(parsed?.task_id);
  if (!taskId) {
    return;
  }
  tool.backgroundTaskId = taskId;
  tool.backgroundRunning = true;
  delete tool.backgroundExitCode;
}

function applyBackgroundTaskFinished(
  session: WebviewSessionSnapshot,
  taskId: string,
  exitCode: number | undefined,
): void {
  const tool = session.timeline.find(
    (item): item is WebviewToolCard =>
      item.type === "tool" && item.backgroundTaskId === taskId,
  );
  if (!tool) {
    return;
  }
  tool.backgroundRunning = false;
  if (typeof exitCode === "number" && Number.isFinite(exitCode)) {
    tool.backgroundExitCode = exitCode;
  } else {
    delete tool.backgroundExitCode;
  }
}

function countCompletedItems(items: unknown): { completed: number; total: number } | undefined {
  if (!Array.isArray(items)) {
    return undefined;
  }
  let completed = 0;
  let total = 0;
  for (const item of items) {
    if (!isRecord(item)) {
      return undefined;
    }
    total += 1;
    if (item.status === "completed") {
      completed += 1;
    }
  }
  return { completed, total };
}

function countTodosFromArgs(todos: unknown): { completed: number; total: number } | undefined {
  if (!Array.isArray(todos)) {
    return undefined;
  }
  let completed = 0;
  for (const todo of todos) {
    if (isRecord(todo) && todo.status === "completed") {
      completed += 1;
    }
  }
  return { completed, total: todos.length };
}

function countCheckedOps(args: Record<string, unknown> | undefined): number {
  const ops = args?.ops;
  if (!Array.isArray(ops)) {
    return 0;
  }
  return ops.reduce((count, op) => {
    if (!isRecord(op)) {
      return count;
    }
    const kind = asNonEmptyString(op.kind);
    const status = asNonEmptyString(op.status);
    if ((kind === "set_status" || kind === "upsert") && status === "completed") {
      return count + 1;
    }
    return count;
  }, 0);
}

function derivePlanReference(
  toolName: string,
  args: Record<string, unknown> | undefined,
  resultText: string | undefined,
): { planId?: string; planPath?: string } {
  if (toolName !== "create_plan" && toolName !== "update_plan") {
    return {};
  }
  const parsed = parseToolSummaryJson(resultText);
  return {
    planId:
      asNonEmptyString(parsed?.plan_id) ??
      asNonEmptyString(args?.plan_id) ??
      asNonEmptyString(args?.planId),
    planPath: asNonEmptyString(parsed?.path) ?? asNonEmptyString(args?.path),
  };
}

export function derivePlanActivity(
  toolName: string,
  resultText: string | undefined,
  args: Record<string, unknown> | undefined,
): WebviewToolCard["planActivity"] | undefined {
  if (toolName !== "create_plan" && toolName !== "update_plan") {
    return undefined;
  }
  const parsed = parseToolSummaryJson(resultText);
  if (!parsed) {
    return undefined;
  }

  if (toolName === "create_plan") {
    const counts = countTodosFromArgs(args?.todos);
    const stateAfter = normalizePlanState(parsed.state);
    return {
      completed: counts?.completed,
      kind: "create",
      stateAfter,
      title: asNonEmptyString(args?.goal) ?? null,
      total: counts?.total,
    };
  }

  const counts = countCompletedItems(parsed.items);
  const applied = asFiniteNumber(parsed.applied);
  const stateBefore = normalizePlanState(parsed.plan_state_before);
  const stateAfter = normalizePlanState(parsed.plan_state_after);
  if (applied === undefined && !counts && !stateBefore && !stateAfter) {
    return undefined;
  }
  return {
    applied,
    checked: countCheckedOps(args),
    completed: counts?.completed,
    kind: "update",
    stateAfter,
    stateBefore,
    total: counts?.total,
  };
}

function applyPlanReference(tool: WebviewToolCard, resultText?: string): void {
  const reference = derivePlanReference(tool.toolName, tool.args, resultText);
  if (reference.planId) {
    tool.planId = reference.planId;
  }
  if (reference.planPath) {
    tool.planPath = reference.planPath;
  }
}

function stampRunningCreatePlan(
  session: WebviewSessionSnapshot,
  path: string,
  planId: string | null | undefined,
): void {
  for (let index = session.timeline.length - 1; index >= 0; index -= 1) {
    const item = session.timeline[index];
    if (item.type !== "tool") {
      continue;
    }
    if (
      item.toolName !== "create_plan" ||
      item.isError ||
      (item.status !== "running" && item.status !== "streaming")
    ) {
      continue;
    }
    item.planPath = path;
    if (planId) {
      item.planId = planId;
    }
    return;
  }
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

  if (state) {
    session.planState = state;
  }
  if (planId) {
    session.planId = planId;
  }

  const syncHistoryPlanRef = () => {
    const nextState = state ?? session.planState ?? null;
    if (path) {
      syncPlanRef(session, path, nextState, planId ?? session.planId ?? null);
      return;
    }
    if (session.planFile) {
      session.planFile = {
        ...session.planFile,
        planId: planId ?? session.planFile.planId ?? null,
        state: nextState ?? session.planFile.state ?? null,
      };
    }
  };

  switch (eventName) {
    case "plan.create":
    case "plan.build":
    case "plan.update":
    case "plan.complete":
    case "plan.pending":
      syncHistoryPlanRef();
      return;
    case "plan.todos": {
      const todos = parseTodos(entry.todos);
      if (todos.length > 0 || Array.isArray(entry.todos)) {
        session.planTodos = todos;
      }
      return;
    }
    case "plan.review":
      if (typeof entry.summary === "string" && entry.summary.length > 0) {
        pushMessage(
          session,
          "notice",
          `Tomcat plan review: ${entry.summary}`,
          planEventMessageId(eventName, planId, entry.summary),
        );
      }
      return;
    case "plan.code_review":
      if (planId) {
        upsertDoneCodeReviewRow(session, {
          aborted: entry.aborted,
          findings: entry.findings,
          planId,
          rounds: entry.rounds,
          summary: entry.summary,
          verdict: entry.verdict,
        });
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

  if (entry.type === "error") {
    const summary =
      typeof entry.summary === "string" && entry.summary.length > 0
        ? entry.summary
        : typeof entry.detail === "string" && entry.detail.length > 0
          ? entry.detail
          : "Unknown error";
    session.timeline.push({
      detailText: typeof entry.detail === "string" ? entry.detail : null,
      id: typeof entry.id === "string" ? entry.id : `history-error-${session.timeline.length + 1}`,
      kind: "error",
      text: summary,
      type: "message",
    } satisfies WebviewMessageBlock);
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
      const segments = contentToMessageSegments(entry.message.content);
      session.timeline.push({
        id,
        kind: "user",
        segments,
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
      const toolName = historyToolNames.get(toolCallId) ?? "tool";
      const planReference = derivePlanReference(toolName, args, text);
      const planActivity = derivePlanActivity(toolName, text, args);
      session.timeline.push({
        args,
        assistantMessageId: toolCallToAssistant.get(toolCallId),
        id,
        isError: false,
        planActivity,
        planId: planReference.planId,
        planPath: planReference.planPath,
        status: "complete",
        summary: text,
        toolCallId,
        toolName,
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

function createSessionRuntime(): SessionRuntimeState {
  return {
    activeAssistantId: null,
    activeThinkingId: null,
    streamingAssistantId: null,
    hasMoreHistory: false,
    historyEntries: [],
    historyLoading: false,
    localUserMessageIds: new Set<string>(),
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

function mergeLatestHistoryEntries(existing: unknown[], latest: unknown[]): unknown[] {
  const latestByKey = new Map<string, unknown>();
  for (const entry of latest) {
    latestByKey.set(historyEntryKey(entry), entry);
  }

  const seen = new Set<string>();
  const merged = existing.map((entry) => {
    const key = historyEntryKey(entry);
    seen.add(key);
    return latestByKey.get(key) ?? entry;
  });

  for (const entry of latest) {
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

/** 把 utility-flash 异步生成的命令"目的"短句写到对应工具卡片（按 toolCallId 定位）。 */
function applyToolSummaryTitle(
  session: WebviewSessionSnapshot,
  toolCallId: string,
  summaryTitle: string,
): void {
  const tool = session.timeline.find(
    (item): item is WebviewToolCard => item.type === "tool" && item.toolCallId === toolCallId,
  );
  if (tool) {
    tool.summaryTitle = summaryTitle;
  }
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
  options: AppendMessageOptions = {},
): WebviewMessageBlock {
  const next: WebviewMessageBlock = {
    id: createTimelineId(session, kind, preferredId),
    kind,
    text,
    type: "message",
  };
  if (options.deliveryError !== undefined) {
    next.deliveryError = options.deliveryError;
  }
  if (options.detailText !== undefined) {
    next.detailText = options.detailText;
  }
  if (options.deliveryState) {
    next.deliveryState = options.deliveryState;
  }
  if (options.retryable !== undefined) {
    next.retryable = options.retryable;
  }
  if (options.submitKind) {
    next.submitKind = options.submitKind;
  }
  if (options.segments?.length) {
    next.segments = options.segments.map((segment) => ({ ...segment }));
  }
  session.timeline.push(next);
  return next;
}

function liveAssistantGroupIds(runtime: SessionRuntimeState): Set<string> {
  const ids = new Set<string>();
  if (runtime.activeAssistantId) {
    ids.add(runtime.activeAssistantId);
  }
  if (runtime.streamingAssistantId) {
    ids.add(runtime.streamingAssistantId);
  }
  return ids;
}

function collectOptimisticTailKeys(
  session: WebviewSessionSnapshot,
  runtime: SessionRuntimeState,
  existingKeys: Set<string>,
): Set<string> {
  const keys = new Set<string>();
  let collecting = false;
  for (let index = session.timeline.length - 1; index >= 0; index -= 1) {
    const item = session.timeline[index];
    if (
      item.type === "message" &&
      item.kind === "user" &&
      runtime.localUserMessageIds.has(item.id) &&
      (item.deliveryState === "pending" || item.deliveryState === "failed")
    ) {
      collecting = true;
    }
    if (!collecting) {
      continue;
    }
    const key = timelineEntityKey(item);
    if (existingKeys.has(key)) {
      break;
    }
    keys.add(key);
  }
  return keys;
}

function shouldRetainLiveTimelineItem(
  item: WebviewTimelineItem,
  runtime: SessionRuntimeState,
  assistantGroupIds: Set<string>,
): boolean {
  switch (item.type) {
    case "message":
      if (item.kind === "user") {
        return (
          runtime.localUserMessageIds.has(item.id) &&
          (item.deliveryState === "pending" || item.deliveryState === "failed")
        );
      }
      return (
        item.kind === "assistant" &&
        typeof item.assistantMessageId === "string" &&
        assistantGroupIds.has(item.assistantMessageId)
      );
    case "thinking":
      return (
        runtime.activeThinkingId === item.id ||
        (typeof item.assistantMessageId === "string" &&
          assistantGroupIds.has(item.assistantMessageId))
      );
    case "tool":
      return (
        item.status === "running" ||
        item.status === "streaming" ||
        (typeof item.assistantMessageId === "string" &&
          assistantGroupIds.has(item.assistantMessageId))
      );
    case "approval":
      return !item.resolved;
    case "review":
      return item.status === "running";
    case "boundary":
    case "checkpoint":
    case "plan":
      return false;
  }
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

function settleRunningTools(session: WebviewSessionSnapshot): void {
  for (const item of session.timeline) {
    if (item.type !== "tool") {
      continue;
    }
    if (item.status === "running" || item.status === "streaming") {
      item.status = "complete";
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
): WebviewSessionTab {
  return {
    busy: effectiveBusy(session.busy, session.interrupted),
    isCurrent: session.isCurrent,
    ownedByThisFrontend: true,
    sessionId: session.sessionId,
    title: session.title,
    updatedAt: session.updatedAt,
  };
}

export class WebviewStateStore {
  private state: WebviewStateSnapshot;
  private readonly runtimes = new Map<string, SessionRuntimeState>();

  constructor() {
    this.state = {
      activeSessionId: null,
      availableModelCapabilities: {},
      availableModelReasoningLevels: {},
      availableModels: [],
      buildModel: "",
      modelAdminSupported: false,
      ready: false,
      sessionViews: {},
      sessions: [],
    };
  }

  snapshot(): WebviewStateSnapshot {
    return cloneSnapshot(this.state);
  }

  setReady(ready: boolean): void {
    this.state.ready = ready;
  }

  setAvailableModels(
    models: string[],
    capabilities: Record<string, string[]> = {},
    reasoningLevels: Record<string, string[]> = {},
  ): void {
    this.state.availableModelCapabilities = { ...capabilities };
    this.state.availableModelReasoningLevels = { ...reasoningLevels };
    this.state.availableModels = [...models];
  }

  setBuildModel(buildModel: string): void {
    this.state.buildModel = buildModel;
  }

  setModelAdminSupported(supported: boolean): void {
    this.state.modelAdminSupported = supported;
  }

  resetForReload(): void {
    this.runtimes.clear();
    this.state = {
      activeSessionId: null,
      availableModelCapabilities: {},
      availableModelReasoningLevels: {},
      availableModels: [],
      buildModel: "",
      modelAdminSupported: false,
      ready: false,
      sessionViews: {},
      sessions: [],
    };
  }

  setActiveSession(sessionId: string | null): void {
    this.state.activeSessionId = sessionId;
    if (sessionId) {
      this.ensureSession(sessionId);
    }
  }

  syncSessionList(payload: SessionListPayload): void {
    this.state.sessions = payload.sessions.map((session) => mapSessionToTab(session));
    if (!this.state.activeSessionId && payload.activeSessionId) {
      this.setActiveSession(payload.activeSessionId);
    }
  }

  applySessionState(
    payload: SessionStatePayload,
    options: {
      trustBusy?: boolean;
    } = {},
  ): void {
    const session = this.ensureSession(payload.sessionId);
    const trustBusy = options.trustBusy ?? true;
    const nextBusy = effectiveBusy(payload.busy, payload.interrupted);
    if (trustBusy) {
      session.busy = nextBusy;
      this.syncTabBusy(payload.sessionId, nextBusy);
    }
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
    } else if (session.planFile) {
      const nextState = session.planState ?? session.planFile.state ?? null;
      const nextPlanId = session.planId ?? session.planFile.planId ?? null;
      session.planFile = {
        ...session.planFile,
        planId: nextPlanId,
        state: nextState,
      };
    }
    session.ownedByThisFrontend = true;
    this.syncTabOwnedByFrontend(payload.sessionId);
  }

  setPendingAttachments(sessionId: string, attachments: WebviewPendingAttachment[]): void {
    this.ensureSession(sessionId).pendingAttachments = [...attachments];
  }

  setCheckpoints(sessionId: string, checkpoints: SessionCheckpointPayload[]): void {
    const session = this.ensureSession(sessionId);
    session.checkpoints = checkpoints.map((checkpoint) => ({
      changedFiles: [...checkpoint.changedFiles],
      createdAt: checkpoint.createdAt,
      id: checkpoint.id,
      kind: checkpoint.kind,
      label: checkpoint.label ?? null,
      messageAnchor: checkpoint.messageAnchor ?? null,
    }));
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
    runtime.historyEntries = mergeLatestHistoryEntries(
      runtime.historyEntries,
      Array.isArray(history.messages) ? history.messages : [],
    );
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
    options: AppendMessageOptions = {},
  ): void {
    if (!text) {
      return;
    }
    pushMessage(this.ensureSession(sessionId), kind, text, options.preferredId, options);
  }

  appendLocalUserMessage(
    sessionId: string,
    text: string,
    options: {
      messageId: string;
      segments?: WebviewMessageSegment[];
      submitKind: UserSubmitKind;
    },
  ): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    pushMessage(session, "user", text, options.messageId, {
      deliveryState: "pending",
      segments: options.segments,
      submitKind: options.submitKind,
    });
    runtime.localUserMessageIds.add(options.messageId);
  }

  markLocalUserMessageFailed(
    sessionId: string,
    messageId: string,
    error: string,
    retryable: boolean,
  ): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "user" && item.id === messageId,
    );
    if (!message) {
      return;
    }
    message.deliveryError = error;
    message.deliveryState = "failed";
    message.retryable = retryable;
    runtime.localUserMessageIds.add(messageId);
  }

  markLocalUserMessagePending(sessionId: string, messageId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "user" && item.id === messageId,
    );
    if (!message) {
      return;
    }
    delete message.deliveryError;
    message.deliveryState = "pending";
    delete message.retryable;
    runtime.localUserMessageIds.add(messageId);
  }

  markLocalUserMessageConfirmed(sessionId: string, messageId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    runtime.localUserMessageIds.delete(messageId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "user" && item.id === messageId,
    );
    if (!message) {
      return;
    }
    delete message.deliveryState;
    delete message.deliveryError;
    delete message.retryable;
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
    if (
      frame.type === "__test.capture_dom" ||
      frame.type === "__test.dom_action" ||
      frame.type === "contextSearchResult" ||
      frame.type === "insertReference"
    ) {
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
        this.syncTabBusy(session.sessionId, true);
        clearActiveAssistant(runtime);
        return;
      case "agent_end":
        clearActiveAssistant(runtime);
        if (frame.error && frame.error !== "interrupted") {
          pushMessage(session, "error", frame.error);
        }
        return;
      case "agent_interrupted":
        clearActiveAssistant(runtime);
        markRunningToolsInterrupted(session);
        if (!messageExistsAtTail(session, "warn", "Tomcat turn interrupted")) {
          pushMessage(session, "warn", "Tomcat turn interrupted");
        }
        return;
      case "agent_idle":
        session.busy = false;
        this.syncTabBusy(session.sessionId, false);
        settleRunningTools(session);
        clearActiveAssistant(runtime);
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
        if (frame.subagentType === "code_reviewer") {
          const planId = activePlanId(session);
          if (planId) {
            upsertRunningCodeReviewRow(session, planId);
            return;
          }
        }
        pushMessage(session, "notice", `Started ${frame.subagentType} sub-agent`);
        return;
      case "sub_agent_end":
        if (frame.subagentType === "code_reviewer") {
          const planId = activePlanId(session);
          if (planId) {
            settleRunningCodeReviewAsAborted(session, planId);
            return;
          }
        }
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
        tool.startedAt = Date.now();
        applyPlanReference(tool);
        delete tool.planActivity;
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
        applyPlanReference(tool);
        return;
      }
      case "tool_execution_end": {
        clearThinkingStreaming(runtime);
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        tool.display = frame.display ?? undefined;
        if (
          frame.display?.kind === "file" &&
          typeof frame.display.added === "number" &&
          typeof frame.display.removed === "number"
        ) {
          tool.diffStat = {
            added: frame.display.added,
            removed: frame.display.removed,
          };
        } else {
          delete tool.diffStat;
        }
        if (frame.display?.kind === "file" && Array.isArray(frame.display.diff)) {
          tool.diff = frame.display.diff;
        } else {
          delete tool.diff;
        }
        tool.isError = frame.isError;
        tool.status = toolResultWasInterrupted(frame.result) ? "interrupted" : "complete";
        tool.summary = toolResultWasInterrupted(frame.result) ? "Interrupted" : asText(frame.result);
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        applyPlanReference(tool, tool.summary);
        applyBackgroundTaskTicket(tool, tool.summary);
        if (!tool.isError && tool.status === "complete") {
          tool.planActivity = derivePlanActivity(tool.toolName, tool.summary, tool.args);
        } else {
          delete tool.planActivity;
        }
        return;
      }
      case "plan.todos": {
        const todos = parseTodos("todos" in frame ? frame.todos : undefined);
        session.planTodos = todos;
        if ("planId" in frame && typeof frame.planId === "string") {
          session.planId = frame.planId;
          if (session.planFile) {
            session.planFile = {
              ...session.planFile,
              planId: frame.planId,
            };
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
      case "tool.summary_updated": {
        const toolCallId =
          "toolCallId" in frame && typeof frame.toolCallId === "string"
            ? frame.toolCallId
            : null;
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (!toolCallId || !summaryTitle) {
          return;
        }
        applyToolSummaryTitle(session, toolCallId, summaryTitle);
        return;
      }
      case "background_task_finished": {
        const taskId =
          "taskId" in frame && typeof frame.taskId === "string" ? frame.taskId : null;
        if (!taskId) {
          return;
        }
        const exitCode =
          "exitCode" in frame && typeof frame.exitCode === "number" ? frame.exitCode : undefined;
        applyBackgroundTaskFinished(session, taskId, exitCode);
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
    const renderableEntries = trimLeadingHistoryEntries(
      filterSupersededHistoryEntries(runtime.historyEntries),
    );
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
    const existingKeys = new Set(historySession.timeline.map((item) => timelineEntityKey(item)));
    const optimisticTailKeys = collectOptimisticTailKeys(session, runtime, existingKeys);
    const assistantGroupIds = liveAssistantGroupIds(runtime);
    const nextLocalUserMessageIds = new Set<string>();
    for (const item of session.timeline) {
      const key = timelineEntityKey(item);
      const trackedLocalUserMessage =
        item.type === "message" &&
        item.kind === "user" &&
        runtime.localUserMessageIds.has(item.id);
      if (existingKeys.has(key)) {
        continue;
      }
      if (
        !optimisticTailKeys.has(key) &&
        !shouldRetainLiveTimelineItem(item, runtime, assistantGroupIds)
      ) {
        continue;
      }
      upsertTimelineItem(historySession, item);
      existingKeys.add(key);
      if (trackedLocalUserMessage) {
        nextLocalUserMessageIds.add(item.id);
      }
    }
    runtime.localUserMessageIds = nextLocalUserMessageIds;
    session.timeline = historySession.timeline;
    if (!session.planFile && historySession.planFile) {
      session.planFile = historySession.planFile;
    } else if (session.planFile && historySession.planFile && session.planFile.path === historySession.planFile.path) {
      session.planFile = {
        ...session.planFile,
        planId: session.planFile.planId ?? historySession.planFile.planId ?? null,
        state: session.planFile.state ?? historySession.planFile.state ?? null,
      };
    }
    if (!session.planId && historySession.planId) {
      session.planId = historySession.planId;
    }
    if (session.planTodos.length === 0 && historySession.planTodos.length > 0) {
      session.planTodos = historySession.planTodos;
    }
    if (
      (!session.planState || session.planState === "chat") &&
      historySession.planState &&
      historySession.planState !== "chat"
    ) {
      session.planState = historySession.planState;
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
      stampRunningCreatePlan(session, event.path, event.planId ?? session.planId ?? null);
    } else if (session.planFile) {
      session.planFile = {
        ...session.planFile,
        planId: event.planId ?? session.planFile.planId ?? null,
        state: state ?? session.planFile.state ?? null,
      };
    }

    switch (event.type) {
      case "plan.review":
        if (event.summary) {
          pushMessage(
            session,
            "notice",
            `Tomcat plan review: ${event.summary}`,
            planEventMessageId(event.type, event.planId, event.summary),
          );
        }
        return;
      case "plan.code_review":
        if (event.planId) {
          upsertDoneCodeReviewRow(session, {
            aborted: event.aborted,
            findings: event.findings,
            planId: event.planId,
            rounds: event.rounds,
            summary: event.summary,
            verdict: event.verdict,
          });
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

  private syncTabOwnedByFrontend(sessionId: string): void {
    const existing = this.state.sessions.find((session) => session.sessionId === sessionId);
    if (existing) {
      existing.ownedByThisFrontend = true;
      return;
    }
    this.state.sessions.push({
      busy: false,
      isCurrent: false,
      ownedByThisFrontend: true,
      sessionId,
      title: null,
      updatedAt: null,
    });
  }

  private syncTabBusy(sessionId: string, busy: boolean): void {
    const existing = this.state.sessions.find((session) => session.sessionId === sessionId);
    if (existing) {
      existing.busy = busy;
      return;
    }
    this.state.sessions.push({
      busy,
      isCurrent: false,
      ownedByThisFrontend: true,
      sessionId,
      title: null,
      updatedAt: null,
    });
  }
}
