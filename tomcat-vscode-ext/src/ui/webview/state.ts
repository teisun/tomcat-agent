import type {
  AskQuestionResult,
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
  normalizePlanFileState,
  planEventState,
  type WebviewPlanFileState,
} from "../../shared/planState";
import {
  INTERRUPTED_TOOL_RESULT_TEXT,
  PENDING_TOOL_RESULT_TEXT,
} from "../../shared/toolResultPlaceholders";
import type {
  HostEventFrameContent,
  WebviewApprovalCard,
  WebviewAttachmentView,
  WebviewBoundaryBlock,
  WebviewConnectionStatus,
  WebviewMessageBlock,
  WebviewMessageSegment,
  WebviewPendingAttachment,
  WebviewPlanFileRef,
  WebviewReviewFinding,
  WebviewReviewRow,
  WebviewSessionPatchOp,
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

function cloneSessionSnapshot(
  session: WebviewSessionSnapshot,
): WebviewSessionSnapshot {
  return JSON.parse(JSON.stringify(session)) as WebviewSessionSnapshot;
}

type SessionRuntimeState = {
  activeAssistantId: string | null;
  activeThinkingId: string | null;
  streamingAssistantId: string | null;
  hasMoreHistory: boolean;
  historyEntries: unknown[];
  historyLoading: boolean;
  dismissedErrorCards: Map<string, WebviewMessageBlock>;
  dismissedErrorIds: Set<string>;
  localUserMessageIds: Set<string>;
  oldestHistoryCursor: string | null;
  turnHadAssistantText: boolean;
};

type UserSubmitKind = "prompt" | "steer";

type AppendMessageOptions = {
  detailText?: string | null;
  deliveryError?: string | null;
  deliveryErrorDetail?: string | null;
  deliveryState?: "failed" | "pending";
  attachments?: NonNullable<WebviewMessageBlock["attachments"]>;
  label?: string | null;
  preferredId?: string | null;
  recoveryAction?: ErrorRecoveryAction;
  recoveryTargetUserMessageId?: string;
  retryable?: boolean;
  segments?: WebviewMessageSegment[];
  submitKind?: UserSubmitKind;
};

export type SessionRenderMutation =
  | {
      kind: "none";
    }
  | {
      kind: "patch";
      ops: WebviewSessionPatchOp[];
      sessionId: string;
    }
  | {
      kind: "session";
      sessionId: string;
    };

const NO_SESSION_RENDER_MUTATION: SessionRenderMutation = { kind: "none" };

function sessionRenderMutation(sessionId: string): SessionRenderMutation {
  return {
    kind: "session",
    sessionId,
  };
}

function patchRenderMutation(
  sessionId: string,
  ops: WebviewSessionPatchOp[],
): SessionRenderMutation {
  if (ops.length === 0) {
    return NO_SESSION_RENDER_MUTATION;
  }
  return {
    kind: "patch",
    ops,
    sessionId,
  };
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

function formatLlmErrorDisplay(input: {
  errorMessage: unknown;
  reason: unknown;
}): { label?: string | null; text: string } {
  const reason =
    typeof input.reason === "string" && input.reason.trim().length > 0
      ? input.reason.trim()
      : null;
  const errorMessage =
    typeof input.errorMessage === "string" && input.errorMessage.trim().length > 0
      ? input.errorMessage.trim()
      : null;

  if (!errorMessage && !reason) {
    return { text: "Unknown error" };
  }
  if (!errorMessage) {
    return { text: reason ?? "Unknown error" };
  }
  if (!reason || reason === "error" || reason === errorMessage) {
    return { text: errorMessage };
  }
  return {
    label: reason,
    text: errorMessage,
  };
}

function parseAskQuestionRequest(
  frame: ControlRequestFrame,
): AskQuestionWireRequest | null {
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
    activePlan: null,
    agentMode: "chat",
    busy: false,
    checkpoints: [],
    composerDraft: {
      segments: [],
      text: "",
    },
    contextRatio: null,
    hasMoreHistory: false,
    historyLoading: false,
    model: null,
    planTodos: [],
    sessionTodos: [],
    thinkingLevel: null,
    ownedByThisFrontend: true,
    pendingAttachments: [],
    sessionId,
    timeline: [],
  };
}

function getAssistantDelta(
  event: ServeEvent,
): { delta: string; kind: string } | null {
  if (
    event.type !== "message_update" ||
    !isRecord(event.assistantMessageEvent)
  ) {
    return null;
  }
  const delta = event.assistantMessageEvent.delta;
  const kind = event.assistantMessageEvent.kind;
  if (typeof delta !== "string" || typeof kind !== "string") {
    return null;
  }
  return { delta, kind };
}

/** Allocates an id only for timeline items without a durable external id. */
function createGeneratedTimelineId(
  session: WebviewSessionSnapshot,
  prefix: string,
): string {
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
      return `approval:${approvalIdentity(item.request)}`;
    case "boundary":
      return `boundary:${item.id}`;
    case "checkpoint":
      return `checkpoint:${item.checkpointId}`;
    case "plan":
      return `plan:${item.planId ?? item.path}`;
    case "review":
      return `review:${item.reviewAttemptId}`;
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
    (entry.customType === "checkpoint.restore" ||
      (isRecord(entry.extra) &&
        entry.extra.customType === "checkpoint.restore"))
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

function isUserOrAssistantMessageEntry(entry: unknown): boolean {
  return (
    isRecord(entry) &&
    entry.type === "message" &&
    isRecord(entry.message) &&
    (entry.message.role === "user" || entry.message.role === "assistant")
  );
}

function historyEntryId(entry: unknown): string | null {
  return isRecord(entry) && typeof entry.id === "string" ? entry.id : null;
}

function filterSupersededHistoryEntries(entries: unknown[]): unknown[] {
  const filtered: unknown[] = [];
  let inSupersededSpan = false;
  for (const entry of entries) {
    if (isSupersededMessageEntry(entry)) {
      if (isTurnFailedMessageEntry(entry)) {
        // Retry is append-only: retain the original failed input as an
        // abandoned bubble beside its copy-forward replacement. It is useful
        // audit context and must survive history rebuilds.
        filtered.push(entry);
        continue;
      }
      // Resume replaces only a synthetic tool result (`[pending]`) under the same
      // declaration. That is a point replacement, not a checkpoint-style span:
      // hiding until the next user message would also hide the real appended result.
      if (messageRole(entry) === "tool") {
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

type ErrorRecoveryAction = "resume" | "retry";

type ErrorRecovery = {
  action: ErrorRecoveryAction;
  targetUserMessageId?: string;
};

function messageRole(entry: unknown): string | null {
  return isRecord(entry) && entry.type === "message" && isRecord(entry.message) &&
    typeof entry.message.role === "string"
    ? entry.message.role
    : null;
}

function systemNoteTitle(message: Record<string, unknown>): string | null {
  switch (message.kind) {
    case "nudge":
      return "计划未收口，已要求继续";
    case "signal":
      return "后台任务已结束";
    case "plan_build":
      return "开始执行计划";
    default:
      return null;
  }
}

function toolCallIds(entry: unknown): string[] {
  if (!isRecord(entry) || entry.type !== "message" || !isRecord(entry.message)) {
    return [];
  }
  const calls = entry.message.tool_calls ?? entry.message.toolCalls;
  if (!Array.isArray(calls)) {
    return [];
  }
  return calls.flatMap((call) =>
    isRecord(call) && typeof call.id === "string" && call.id.trim() ? [call.id] : [],
  );
}

function toolResultId(entry: unknown): string | null {
  if (!isRecord(entry) || entry.type !== "message" || !isRecord(entry.message)) {
    return null;
  }
  const value = entry.message.tool_call_id ?? entry.message.toolCallId;
  return typeof value === "string" && value.trim() ? value : null;
}

/**
 * Error entries have no direct turn id. The transcript's linear ordering is the durable
 * relation: find the nearest user input, then inspect tool calls in that turn. No calls
 * means replaying the prompt is safe (Retry); all calls paired with results means the
 * transcript itself is ready for a no-input Resume. Incomplete calls are Batch 7's
 * pending-tool recovery and deliberately receive no misleading button here.
 */
function isCurrentErrorEntry(entries: unknown[], errorIndex: number): boolean {
  for (let index = errorIndex + 1; index < entries.length; index += 1) {
    const entry = entries[index];
    if (isRecord(entry) && entry.type === "error") {
      return false;
    }
    // A later user or assistant row means this failure has already been superseded by a
    // newer attempt. A post-error tool result is hydration repair, not a new turn, and must
    // leave this error visible so the user can still Resume.
    if (isUserOrAssistantMessageEntry(entry)) {
      return false;
    }
  }
  return true;
}

function buildErrorRecoveryActions(
  entries: unknown[],
  sessionBusy: boolean,
  dismissedErrorIds: ReadonlySet<string>,
): Map<string, ErrorRecovery> {
  const actions = new Map<string, ErrorRecovery>();
  if (sessionBusy) {
    return actions;
  }
  for (let errorIndex = 0; errorIndex < entries.length; errorIndex += 1) {
    const error = entries[errorIndex];
    const errorId = historyEntryId(error);
    if (
      !errorId ||
      !isRecord(error) ||
      error.type !== "error" ||
      dismissedErrorIds.has(errorId) ||
      !isCurrentErrorEntry(entries, errorIndex)
    ) {
      continue;
    }
    let userIndex = errorIndex - 1;
    while (userIndex >= 0 && messageRole(entries[userIndex]) !== "user") {
      userIndex -= 1;
    }
    if (userIndex < 0) {
      continue;
    }
    const targetUserMessageId = historyEntryId(entries[userIndex]);
    const calls = new Set<string>();
    const results = new Set<string>();
    // Hydration may repair a dangling tool call by appending its synthetic result after the
    // error anchor. Scan through the current history tail so that repaired Shape C becomes
    // a truthful Resume instead of an unrecoverable dead end.
    for (let index = userIndex + 1; index < entries.length; index += 1) {
      for (const callId of toolCallIds(entries[index])) calls.add(callId);
      if (messageRole(entries[index]) === "tool") {
        const resultId = toolResultId(entries[index]);
        if (resultId) results.add(resultId);
      }
    }
    if (calls.size === 0 && targetUserMessageId) {
      actions.set(errorId, { action: "retry", targetUserMessageId });
    } else if (calls.size > 0 && [...calls].every((callId) => results.has(callId))) {
      actions.set(errorId, {
        action: "resume",
        targetUserMessageId: targetUserMessageId ?? undefined,
      });
    }
  }
  return actions;
}

function currentTurnRecoveryAction(
  session: WebviewSessionSnapshot,
): ErrorRecovery | undefined {
  let userIndex = -1;
  for (let index = session.timeline.length - 1; index >= 0; index -= 1) {
    const item = session.timeline[index];
    if (item.type === "message" && item.kind === "user") {
      userIndex = index;
      break;
    }
  }
  if (userIndex < 0) {
    return undefined;
  }

  const tools = session.timeline.slice(userIndex + 1).filter(
    (item): item is WebviewToolCard => item.type === "tool",
  );
  if (tools.length === 0) {
    return {
      action: "retry",
      targetUserMessageId: session.timeline[userIndex]?.id,
    };
  }
  return tools.every((tool) => tool.status === "complete")
    ? {
      action: "resume",
      targetUserMessageId: session.timeline[userIndex]?.id,
    }
    : undefined;
}

function planEventMessageId(
  eventType: string,
  planId: string | null | undefined,
  detail: string | null | undefined,
): string {
  return `plan-event:${eventType}:${planId ?? "none"}:${detail && detail.length > 0 ? detail : "default"}`;
}

function upsertPlanEventMessage(
  session: WebviewSessionSnapshot,
  kind: "notice" | "warn",
  text: string,
  eventType: string,
  planId: string | null | undefined,
  detail: string | null | undefined,
): void {
  upsertTimelineItem(session, {
    id: planEventMessageId(eventType, planId, detail),
    kind,
    text,
    type: "message",
  });
}

function activePlanId(session: WebviewSessionSnapshot): string | null {
  return session.activePlan?.planId ?? null;
}

function parseReviewVerdict(
  value: unknown,
): WebviewReviewRow["verdict"] | undefined {
  return value === "pass" ||
    value === "fail" ||
    value === "partial" ||
    value === "aborted" ||
    value === "skipped"
    ? value
    : undefined;
}

function skippedReviewSummary(value: unknown): string {
  switch (value) {
    case "no_reviewable_code_diff":
      return "no code changes";
    case "reviewer_unavailable":
      return "reviewer not configured";
    case "workspace_unavailable":
      return "workspace unavailable";
    default:
      return "review skipped";
  }
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

function reviewAttemptId(
  planId: string,
  round: unknown,
  explicit: unknown,
): string {
  if (typeof explicit === "string" && explicit.length > 0) {
    return explicit;
  }
  return `${planId}:${typeof round === "number" ? round : 1}`;
}

function parseTimestampMs(value: unknown): number | undefined {
  if (typeof value !== "string" || value.trim().length === 0) {
    return undefined;
  }
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function upsertRunningCodeReviewRow(
  session: WebviewSessionSnapshot,
  input: {
    planId: string;
    reviewAttemptId?: unknown;
    round?: unknown;
    startedAt?: unknown;
    toolCallId?: unknown;
  },
): void {
  const attemptId = reviewAttemptId(
    input.planId,
    input.round,
    input.reviewAttemptId,
  );
  const existing = session.timeline.find(
    (item): item is WebviewReviewRow =>
      item.type === "review" && item.reviewAttemptId === attemptId,
  );
  if (existing?.status === "done") {
    return;
  }
  const startedAt =
    asFiniteNumber(input.startedAt) ?? existing?.startedAt ?? Date.now();
  upsertTimelineItem(session, {
    anchorToolCallId:
      typeof input.toolCallId === "string" ? input.toolCallId : null,
    id: `review:${attemptId}`,
    planId: input.planId,
    reviewAttemptId: attemptId,
    round: typeof input.round === "number" ? input.round : null,
    startedAt,
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
    reviewAttemptId?: unknown;
    round?: unknown;
    rounds?: unknown;
    skipReason?: unknown;
    toolCallId?: unknown;
    summary?: unknown;
    verdict?: unknown;
  },
): void {
  const verdict =
    parseReviewVerdict(input.verdict) ??
    (input.aborted === true ? "aborted" : undefined);
  const round = typeof input.round === "number" ? input.round : input.rounds;
  const attemptId = reviewAttemptId(input.planId, round, input.reviewAttemptId);
  const existing = session.timeline.find(
    (item): item is WebviewReviewRow =>
      item.type === "review" && item.reviewAttemptId === attemptId,
  );
  upsertTimelineItem(session, {
    anchorToolCallId:
      typeof input.toolCallId === "string" ? input.toolCallId : null,
    findings: parseReviewFindings(input.findings),
    id: `review:${attemptId}`,
    planId: input.planId,
    reviewAttemptId: attemptId,
    round: typeof round === "number" ? round : null,
    rounds: typeof round === "number" ? round : null,
    startedAt: existing?.startedAt,
    status: "done",
    summary:
      verdict === "skipped"
        ? skippedReviewSummary(input.skipReason)
        : typeof input.summary === "string"
          ? input.summary
          : null,
    type: "review",
    verdict,
  } satisfies WebviewReviewRow);
}

/** A review dispatch has no durable "running" state. If its parent turn ends
 * without a terminal review event, render the orphaned row as aborted rather
 * than leaving a timer running indefinitely. */
function abortRunningCodeReviewRows(session: WebviewSessionSnapshot): void {
  for (const item of session.timeline) {
    if (item.type !== "review" || item.status !== "running") {
      continue;
    }
    item.status = "done";
    item.verdict = "aborted";
    item.summary = item.summary ?? "Code review was interrupted before completion.";
  }
}

function cloneTimelineItem<T extends WebviewTimelineItem>(item: T): T {
  return JSON.parse(JSON.stringify(item)) as T;
}

function timelineNeighbors(
  session: WebviewSessionSnapshot,
  itemId: string,
): { afterId?: string | null; beforeId?: string | null } {
  const index = session.timeline.findIndex((item) => item.id === itemId);
  if (index < 0) {
    return {};
  }
  return {
    afterId: index > 0 ? session.timeline[index - 1]?.id ?? null : null,
    beforeId:
      index >= 0 && index < session.timeline.length - 1
        ? session.timeline[index + 1]?.id ?? null
        : null,
  };
}

function buildUpsertPatchOpForItem(
  session: WebviewSessionSnapshot,
  item: WebviewTimelineItem | undefined,
): Extract<WebviewSessionPatchOp, { type: "upsert" }> | null {
  if (!item) {
    return null;
  }
  const neighbors = timelineNeighbors(session, item.id);
  return {
    ...neighbors,
    item: cloneTimelineItem(item),
    type: "upsert",
  };
}

function buildUpsertPatchOpById(
  session: WebviewSessionSnapshot,
  itemId: string,
): Extract<WebviewSessionPatchOp, { type: "upsert" }> | null {
  const item = session.timeline.find((entry) => entry.id === itemId);
  return buildUpsertPatchOpForItem(session, item);
}

function reorderAnchoredReviewRows(session: WebviewSessionSnapshot): void {
  const anchored = session.timeline.filter(
    (item): item is WebviewReviewRow =>
      item.type === "review" && Boolean(item.anchorToolCallId),
  );
  for (const review of anchored) {
    const currentIndex = session.timeline.findIndex(
      (item) => timelineEntityKey(item) === timelineEntityKey(review),
    );
    const anchorIndex = session.timeline.findIndex(
      (item) =>
        item.type === "tool" && item.toolCallId === review.anchorToolCallId,
    );
    if (currentIndex < 0 || anchorIndex < 0 || currentIndex === anchorIndex + 1)
      continue;
    session.timeline.splice(currentIndex, 1);
    const refreshedAnchorIndex = session.timeline.findIndex(
      (item) =>
        item.type === "tool" && item.toolCallId === review.anchorToolCallId,
    );
    session.timeline.splice(refreshedAnchorIndex + 1, 0, review);
  }
}

function upsertTimelineItem(
  session: WebviewSessionSnapshot,
  item: WebviewTimelineItem,
): void {
  const key = timelineEntityKey(item);
  const existingIndex = session.timeline.findIndex(
    (entry) => timelineEntityKey(entry) === key,
  );
  if (existingIndex >= 0) {
    session.timeline[existingIndex] = cloneTimelineItem(item);
  } else {
    session.timeline.push(cloneTimelineItem(item));
  }
  reorderAnchoredReviewRows(session);
}

function pushTextSegment(
  segments: WebviewMessageSegment[],
  text: string,
): void {
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

function contentToMessageSegments(
  content: unknown,
): WebviewMessageSegment[] | undefined {
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
              lineEnd:
                typeof entry.line_end === "number" ? entry.line_end : null,
              lineStart:
                typeof entry.line_start === "number" ? entry.line_start : null,
              path: entry.path,
              text: typeof entry.text === "string" ? entry.text : null,
              type: "reference",
            });
          }
          break;
        case "input_image":
        case "input_image_ref":
        case "image":
          // Attachments are extracted separately onto the block.
          // Skip placeholder text here — thumbnails are rendered by MessageBubble.
          break;
        case "input_file":
        case "file":
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
          lineEnd:
            typeof content.line_end === "number" ? content.line_end : null,
          lineStart:
            typeof content.line_start === "number" ? content.line_start : null,
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
      .map((segment) =>
        segment.type === "text" ? segment.text : segment.label,
      )
      .join("");
    return text || undefined;
  }
  // Structured arrays can legitimately contain images only. Do not stringify
  // their base64 payload into the visible transcript.
  return Array.isArray(content) ? undefined : asText(content);
}

function attachmentFilename(index: number, mimeType: string): string {
  if (mimeType === "application/pdf") {
    return `attachment-${index + 1}.pdf`;
  }
  const extension =
    mimeType === "image/jpeg"
      ? "jpg"
      : mimeType === "image/svg+xml"
        ? "svg"
        : mimeType.split("/").pop() || "png";
  return `image-${index + 1}.${extension}`;
}

/**
 * Read attachment references out of a transcript message.
 *
 * The backend is asked for history with `attachmentMode: "reference"`, so every
 * `input_image` part arrives carrying a `blobSha` and no bytes. This function used to
 * pull `image_b64` out and carry it around in the snapshot, which meant every state
 * push re-sent the whole transcript's worth of base64 and pinned another copy of it on
 * the JavaScript heap.
 *
 * A part with no `blobSha` is skipped rather than guessed at. That happens only if
 * something asked for inline mode by mistake, and quietly reviving the base64 path
 * would hide the mistake instead of surfacing it.
 */
function extractAttachments(
  content: unknown,
  messageId: string,
): WebviewAttachmentView[] {
  if (!Array.isArray(content)) {
    return [];
  }
  const attachments: WebviewAttachmentView[] = [];
  for (let index = 0; index < content.length; index += 1) {
    const entry = content[index];
    if (!isRecord(entry)) continue;
    if (
      entry.type !== "input_image" &&
      entry.type !== "input_image_ref" &&
      entry.type !== "image" &&
      entry.type !== "input_file" &&
      entry.type !== "file"
    ) {
      continue;
    }
    if (typeof entry.blobSha !== "string" || !/^[0-9a-f]{64}$/.test(entry.blobSha)) {
      continue;
    }
    const kind = entry.type === "input_file" || entry.type === "file" ? "file" : "image";
    const mimeType =
      typeof entry.mime_type === "string"
        ? entry.mime_type
        : typeof entry.mimeType === "string"
          ? entry.mimeType
          : kind === "file"
            ? "application/pdf"
            : "image/png";
    attachments.push({
      blobSha: entry.blobSha,
      bytes: typeof entry.bytes === "number" ? entry.bytes : undefined,
      filename:
        typeof entry.filename === "string"
          ? entry.filename
          : attachmentFilename(index, mimeType),
      hasThumb: entry.hasThumb === true,
      id: `${messageId}:${kind}:${index}`,
      kind,
      mimeType,
      path:
        typeof entry.sourcePath === "string"
          ? entry.sourcePath
          : typeof entry.path === "string"
            ? entry.path
            : null,
    });
  }
  return attachments;
}

function extractThinkingText(
  message: Record<string, unknown>,
): string | undefined {
  if (
    typeof message.thinking_text === "string" &&
    message.thinking_text.trim()
  ) {
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

function extractSummaryTitle(
  message: Record<string, unknown>,
): string | undefined {
  if (
    typeof message.summary_title === "string" &&
    message.summary_title.trim()
  ) {
    return message.summary_title.trim();
  }
  return undefined;
}

function extractToolCallId(
  message: Record<string, unknown>,
): string | undefined {
  return typeof message.tool_call_id === "string"
    ? message.tool_call_id
    : undefined;
}

function extractToolDisplay(
  message: Record<string, unknown>,
): WebviewToolCard["display"] | undefined {
  const display = message.tool_display;
  if (!isRecord(display) || typeof display.kind !== "string") {
    return undefined;
  }
  switch (display.kind) {
    case "file":
      return typeof display.file === "string"
        ? (display as unknown as WebviewToolCard["display"])
        : undefined;
    case "files":
      return Array.isArray(display.files)
        ? (display as unknown as WebviewToolCard["display"])
        : undefined;
    case "plan":
      return typeof display.plan === "string"
        ? (display as unknown as WebviewToolCard["display"])
        : undefined;
    case "text":
      return typeof display.text === "string"
        ? (display as unknown as WebviewToolCard["display"])
        : undefined;
    default:
      return undefined;
  }
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

export function buildToolCallToAssistantMap(
  entries: unknown[],
): Map<string, string> {
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

function buildHistoryToolArgsLookup(
  entries: unknown[],
): Map<string, Record<string, unknown>> {
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

function buildHistoryToolResultIds(entries: unknown[]): Set<string> {
  const ids = new Set<string>();
  for (const entry of entries) {
    if (
      isRecord(entry) &&
      entry.type === "message" &&
      isRecord(entry.message) &&
      entry.message.role === "tool"
    ) {
      const toolCallId = extractToolCallId(entry.message);
      if (toolCallId) ids.add(toolCallId);
    }
  }
  return ids;
}

function legacyAskQuestionPayload(entry: Record<string, unknown>): {
  questions: unknown[];
  result: Record<string, unknown>;
  toolCallId: string;
} | null {
  const event = typeof entry.event === "string" ? entry.event : "";
  if (![
    "ask_question",
    "ask_question.result",
    "plan.ask_question",
    "plan.ask_question.result",
  ].includes(event)) return null;
  const payload = isRecord(entry.payload) ? entry.payload : entry;
  const request = isRecord(payload.request) ? payload.request : payload;
  const questions = Array.isArray(request.questions) ? request.questions : null;
  const result = isRecord(payload.result) ? payload.result : null;
  const toolCallId =
    asNonEmptyString(entry.tool_call_id) ??
    asNonEmptyString(entry.toolCallId) ??
    asNonEmptyString(payload.tool_call_id) ??
    asNonEmptyString(payload.toolCallId);
  return questions && result && toolCallId ? { questions, result, toolCallId } : null;
}

function applyLegacyAskQuestionCustomEntry(
  session: WebviewSessionSnapshot,
  entry: Record<string, unknown>,
  historyToolArgs: Map<string, Record<string, unknown>>,
  standardToolResultIds: Set<string>,
  toolCallToAssistant: Map<string, string>,
): boolean {
  const legacy = legacyAskQuestionPayload(entry);
  if (!legacy) return false;
  if (standardToolResultIds.has(legacy.toolCallId)) return true;
  const id = asNonEmptyString(entry.id) ?? `legacy-ask-question-${legacy.toolCallId}`;
  session.timeline.push({
    args: historyToolArgs.get(legacy.toolCallId) ?? { questions: legacy.questions },
    assistantMessageId: toolCallToAssistant.get(legacy.toolCallId),
    id,
    isError: false,
    status: "complete",
    summary: JSON.stringify(legacy.result),
    toolCallId: legacy.toolCallId,
    toolName: "ask_question",
    type: "tool",
  });
  return true;
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
  return typeof value === "string" && value.trim().length > 0
    ? value
    : undefined;
}

function asFiniteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function parseToolSummaryJson(
  resultText: string | undefined,
): Record<string, unknown> | undefined {
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

function applyBackgroundTaskTicket(
  tool: WebviewToolCard,
  resultText?: string,
): void {
  const parsed = parseToolSummaryJson(resultText);
  const resultRunsInBackground = parsed?.state === "running_in_background";
  const runsInBackground =
    tool.args?.run_in_background === true ||
    tool.args?.runInBackground === true ||
    resultRunsInBackground;
  if (
    !runsInBackground ||
    (tool.toolName !== "bash" &&
      tool.toolName !== "shell" &&
      tool.toolName !== "execute_command")
  ) {
    delete tool.backgroundTaskId;
    delete tool.backgroundRunning;
    delete tool.backgroundExitCode;
    return;
  }
  const taskId =
    asNonEmptyString(parsed?.taskId) ?? asNonEmptyString(parsed?.task_id);
  if (!taskId) {
    return;
  }
  tool.backgroundTaskId = taskId;
  tool.backgroundRunning = true;
  tool.logPath =
    asNonEmptyString(parsed?.logPath) ??
    asNonEmptyString(parsed?.log_path) ??
    tool.logPath;
  delete tool.backgroundExitCode;
}

const LIVE_OUTPUT_MAX_LINES = 500;
const LIVE_OUTPUT_MAX_CHARS = 30_000;

function safeUnicodeSuffix(value: string, maxCodeUnits: number): string {
  if (value.length <= maxCodeUnits) return value;
  let start = value.length - maxCodeUnits;
  const current = value.charCodeAt(start);
  const previous = value.charCodeAt(start - 1);
  if (
    current >= 0xdc00 &&
    current <= 0xdfff &&
    previous >= 0xd800 &&
    previous <= 0xdbff
  ) {
    start += 1;
  }
  return value.slice(start);
}

function trimLiveOutput(value: string): { output: string; truncated: boolean } {
  let output = value.replace(/\r\n/g, "\n");
  let truncated = output.length > LIVE_OUTPUT_MAX_CHARS;
  if (truncated) {
    output = safeUnicodeSuffix(output, LIVE_OUTPUT_MAX_CHARS);
  }
  const lines = output.split("\n");
  if (lines.length > LIVE_OUTPUT_MAX_LINES) {
    output = lines.slice(-LIVE_OUTPUT_MAX_LINES).join("\n");
    truncated = true;
  }
  return { output, truncated };
}

function applyLiveToolOutput(
  tool: WebviewToolCard,
  partialResult: unknown,
): void {
  if (
    (tool.toolName !== "bash" &&
      tool.toolName !== "shell" &&
      tool.toolName !== "execute_command") ||
    !isRecord(partialResult) ||
    (partialResult.kind !== undefined && partialResult.kind !== "live_output")
  )
    return;
  const output =
    typeof partialResult.output === "string" ? partialResult.output : undefined;
  const startOffset = asFiniteNumber(partialResult.startOffset);
  const nextOffset = asFiniteNumber(partialResult.nextOffset);
  const sequence = asFiniteNumber(partialResult.sequence);
  if (
    output === undefined ||
    startOffset === undefined ||
    nextOffset === undefined ||
    sequence === undefined ||
    !Number.isInteger(startOffset) ||
    !Number.isInteger(nextOffset) ||
    !Number.isInteger(sequence) ||
    startOffset < 0 ||
    nextOffset < startOffset ||
    sequence < 0
  )
    return;
  if (
    tool.liveOutputSequence !== undefined &&
    sequence <= tool.liveOutputSequence
  )
    return;

  const contiguous =
    tool.liveOutputOffset === undefined
      ? startOffset === 0
      : startOffset === tool.liveOutputOffset;
  const combined = contiguous ? `${tool.liveOutput ?? ""}${output}` : output;
  const trimmed = trimLiveOutput(combined);
  tool.liveOutput = trimmed.output;
  tool.liveOutputOffset = nextOffset;
  tool.liveOutputSequence = sequence;
  tool.liveOutputTruncated =
    trimmed.truncated || partialResult.truncated === true || !contiguous;
  tool.logPath = asNonEmptyString(partialResult.logPath) ?? tool.logPath;
  tool.backgroundTaskId =
    asNonEmptyString(partialResult.taskId) ?? tool.backgroundTaskId;
}

function applyToolDisplay(
  tool: WebviewToolCard,
  display: WebviewToolCard["display"] | null | undefined,
): void {
  tool.display = display ?? undefined;
  if (
    display?.kind === "file" &&
    typeof display.added === "number" &&
    typeof display.removed === "number"
  ) {
    tool.diffStat = {
      added: display.added,
      removed: display.removed,
    };
  } else {
    delete tool.diffStat;
  }
  if (display?.kind === "file" && Array.isArray(display.diff)) {
    tool.diff = display.diff;
  } else {
    delete tool.diff;
  }
  if (display?.kind === "file" && display.diffTruncated === true) {
    tool.diffTruncated = true;
  } else {
    delete tool.diffTruncated;
  }
  if (display?.kind === "file" && display.expired === true) {
    tool.diffExpired = true;
  } else {
    delete tool.diffExpired;
  }
}

function applyBackgroundTaskFinished(
  session: WebviewSessionSnapshot,
  taskId: string,
  exitCode: number | undefined,
): WebviewToolCard | undefined {
  const tool = session.timeline.find(
    (item): item is WebviewToolCard =>
      item.type === "tool" && item.backgroundTaskId === taskId,
  );
  if (!tool) {
    return undefined;
  }
  tool.backgroundRunning = false;
  if (typeof exitCode === "number" && Number.isFinite(exitCode)) {
    tool.backgroundExitCode = exitCode;
  } else {
    delete tool.backgroundExitCode;
  }
  return tool;
}

function countCompletedItems(
  items: unknown,
): { completed: number; total: number } | undefined {
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

function countTodosFromArgs(
  todos: unknown,
): { completed: number; total: number } | undefined {
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
    if (
      (kind === "set_status" || kind === "upsert") &&
      status === "completed"
    ) {
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
    const stateAfter = normalizePlanFileState(parsed.state);
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
  const stateBefore = normalizePlanFileState(parsed.plan_state_before);
  const stateAfter = normalizePlanFileState(parsed.plan_state_after);
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
    normalizePlanFileState(entry.state) ??
    planEventState({ type: eventName } as ServePlanEvent);

  const syncHistoryPlanRef = () => {
    const nextState = state ?? session.activePlan?.state ?? null;
    if (path) {
      syncPlanRef(session, path, nextState, planId ?? session.activePlan?.planId ?? null);
      return;
    }
    if (session.activePlan) {
      session.activePlan = {
        ...session.activePlan,
        planId: planId ?? session.activePlan.planId ?? null,
        state: nextState ?? session.activePlan.state ?? null,
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
        upsertPlanEventMessage(
          session,
          "notice",
          `Tomcat plan review: ${entry.summary}`,
          eventName,
          planId,
          entry.summary,
        );
      }
      return;
    case "plan.code_review.started":
      if (planId) {
        upsertRunningCodeReviewRow(session, {
          planId,
          reviewAttemptId: entry.review_attempt_id ?? entry.reviewAttemptId,
          round: entry.round,
          startedAt: parseTimestampMs(entry.timestamp),
          toolCallId: entry.tool_call_id ?? entry.toolCallId,
        });
      }
      return;
    case "plan.code_review":
      if (planId) {
        upsertDoneCodeReviewRow(session, {
          aborted: entry.aborted,
          findings: entry.findings,
          planId,
          reviewAttemptId: entry.review_attempt_id ?? entry.reviewAttemptId,
          round: entry.round,
          rounds: entry.rounds,
          skipReason: entry.skip_reason ?? entry.skipReason,
          toolCallId: entry.tool_call_id ?? entry.toolCallId,
          summary: entry.summary,
          verdict: entry.verdict,
        });
      }
      return;
    case "plan.verify":
      if (typeof entry.verdict === "string" && entry.verdict.length > 0) {
        upsertPlanEventMessage(
          session,
          "notice",
          `Tomcat plan verify: ${entry.verdict}`,
          eventName,
          planId,
          entry.verdict,
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
        upsertPlanEventMessage(
          session,
          "warn",
          `Tomcat plan warning: ${reason}`,
          eventName,
          planId,
          reason,
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
  standardToolResultIds: Set<string>,
  errorRecoveryActions: ReadonlyMap<string, ErrorRecovery>,
  branchSummaryTexts: ReadonlyMap<string, string>,
): void {
  if (!isRecord(entry) || typeof entry.type !== "string") {
    return;
  }

  if (entry.type === "branch_summary") {
    if (entry.isBoundary !== true) {
      return;
    }
    const id =
      typeof entry.id === "string"
        ? entry.id
        : `boundary-${session.timeline.length + 1}`;
    const summary =
      typeof entry.summary === "string"
        ? entry.summary
        : branchSummaryTexts.get(id) ?? null;
    if (summary === null) {
      // The marker was appended before its asynchronous LLM result was ready. Until a linked
      // branch_summary_text arrives, it is intentionally invisible rather than an empty card.
      return;
    }
    session.timeline.push({
      coveredCount:
        typeof entry.coveredCount === "number" ? entry.coveredCount : null,
      id,
      summary,
      type: "boundary",
    } satisfies WebviewBoundaryBlock);
    return;
  }

  if (entry.type === "branch_summary_text") {
    // The payload belongs at its earlier marker's physical position; it never owns a timeline row.
    return;
  }

  if (entry.type === "error") {
    const id =
      typeof entry.id === "string"
        ? entry.id
        : `history-error-${session.timeline.length + 1}`;
    const recovery = errorRecoveryActions.get(id);
    const summary =
      typeof entry.summary === "string" && entry.summary.length > 0
        ? entry.summary
        : typeof entry.detail === "string" && entry.detail.length > 0
          ? entry.detail
          : "Unknown error";
    session.timeline.push({
      detailText: typeof entry.detail === "string" ? entry.detail : null,
      failureDomain: typeof entry.failureDomain === "string" ? entry.failureDomain : null,
      failureKind: typeof entry.failureKind === "string" ? entry.failureKind : null,
      id,
      kind: "error",
      recoveryAction: recovery?.action,
      ...(recovery?.action === "retry" && recovery.targetUserMessageId
        ? { recoveryTargetUserMessageId: recovery.targetUserMessageId }
        : {}),
      statusCode: typeof entry.statusCode === "number" ? entry.statusCode : null,
      text: summary,
      type: "message",
    } satisfies WebviewMessageBlock);
    return;
  }

  if (entry.type === "message" && isRecord(entry.message)) {
    const role =
      typeof entry.message.role === "string" ? entry.message.role : null;
    const text = extractMessageText(entry.message.content);
    const id =
      typeof entry.id === "string"
        ? entry.id
        : `history-message-${(text ?? role ?? "unknown").length}`;
    if (role === "user") {
      if (entry.message.kind === "compaction_summary") {
        session.timeline.push({
          coveredCount:
            typeof entry.message.coveredCount === "number"
              ? entry.message.coveredCount
              : typeof entry.message.covered_count === "number"
                ? entry.message.covered_count
                : null,
          id,
          summary: text ?? "",
          type: "boundary",
        } satisfies WebviewBoundaryBlock);
        return;
      }
      const noteTitle = systemNoteTitle(entry.message);
      if (noteTitle) {
        session.timeline.push({
          id,
          summary: text ?? "",
          title: noteTitle,
          type: "boundary",
        } satisfies WebviewBoundaryBlock);
        return;
      }
      if (!text && !Array.isArray(entry.message.content)) {
        return;
      }
      const segments = contentToMessageSegments(entry.message.content);
      const attachments = extractAttachments(entry.message.content, id);
      const block: WebviewMessageBlock = {
        ...(entry.message.superseded === true && entry.message.turn_failed === true
          ? { abandoned: true }
          : {}),
        id,
        kind: "user",
        segments,
        text: text ?? "",
        type: "message",
      };
      if (attachments.length > 0) {
        block.attachments = attachments;
      }
      session.timeline.push(block);
      return;
    }
    if (role === "assistant") {
      const hasToolCalls =
        Array.isArray(entry.message.tool_calls) &&
        entry.message.tool_calls.length > 0;
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
      if (isPendingAskQuestionResult(toolName, text)) {
        const request = pendingApprovalRequest(session.sessionId, toolCallId, args);
        if (request) {
          upsertApproval(session, request, session.sessionId, false);
        }
        return;
      }
      if (toolName === "ask_question") {
        resolveApprovalByToolCallId(session, toolCallId);
      }
      const planReference = derivePlanReference(toolName, args, text);
      const planActivity = derivePlanActivity(toolName, text, args);
      const tool: WebviewToolCard = {
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
      };
      applyToolDisplay(tool, extractToolDisplay(entry.message));
      session.timeline.push(tool);
      return;
    }
  }

  if (
    entry.type === "thinking_trace" &&
    typeof entry.text === "string" &&
    entry.text.trim()
  ) {
    session.timeline.push({
      id:
        typeof entry.id === "string"
          ? entry.id
          : `thinking-${entry.text.length}`,
      text: entry.text,
      type: "thinking",
    } satisfies WebviewThinkingBlock);
    return;
  }

  if (entry.type === "custom") {
    if (entry.event === "agent.interrupted") {
      abortRunningCodeReviewRows(session);
      const preferredId =
        typeof entry.id === "string"
          ? `agent-interrupted:${entry.id}`
          : undefined;
      pushMessage(session, "warn", "Tomcat turn interrupted", preferredId);
      return;
    }
    if (
      applyLegacyAskQuestionCustomEntry(
        session,
        entry,
        historyToolArgs,
        standardToolResultIds,
        toolCallToAssistant,
      )
    ) {
      return;
    }
    if (
      entry.event === "session.agent_mode.changed" &&
      (entry.agentMode === "chat" || entry.agentMode === "plan")
    ) {
      session.agentMode = entry.agentMode;
      return;
    }
    applyHistoryPlanCustomEntry(session, entry);
  }
}

function buildBranchSummaryTextLookup(entries: unknown[]): Map<string, string> {
  const summaries = new Map<string, string>();
  for (const entry of entries) {
    if (
      !isRecord(entry) ||
      entry.type !== "branch_summary_text" ||
      typeof entry.forId !== "string" ||
      typeof entry.summary !== "string"
    ) {
      continue;
    }
    summaries.set(entry.forId, entry.summary);
  }
  return summaries;
}

function createSessionRuntime(): SessionRuntimeState {
  return {
    activeAssistantId: null,
    activeThinkingId: null,
    streamingAssistantId: null,
    hasMoreHistory: false,
    historyEntries: [],
    historyLoading: false,
    dismissedErrorCards: new Map<string, WebviewMessageBlock>(),
    dismissedErrorIds: new Set<string>(),
    localUserMessageIds: new Set<string>(),
    oldestHistoryCursor: null,
    turnHadAssistantText: false,
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

function mergeLatestHistoryEntries(
  existing: unknown[],
  latest: unknown[],
): unknown[] {
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
  state: WebviewPlanFileState | null,
  planId?: string | null,
): void {
  session.activePlan = {
    path,
    planId: planId ?? session.activePlan?.planId ?? null,
    state,
  } satisfies WebviewPlanFileRef;
}

function findTimelineItem<T extends WebviewTimelineItem["type"]>(
  session: WebviewSessionSnapshot,
  id: string,
  type: T,
): Extract<WebviewTimelineItem, { type: T }> | undefined {
  return session.timeline.find(
    (item): item is Extract<WebviewTimelineItem, { type: T }> =>
      item.id === id && item.type === type,
  );
}

function findTimelineIndex<T extends WebviewTimelineItem["type"]>(
  session: WebviewSessionSnapshot,
  id: string,
  type: T,
): number {
  return session.timeline.findIndex(
    (item) => item.id === id && item.type === type,
  );
}

function findThinkingByAssistantMessageId(
  session: WebviewSessionSnapshot,
  assistantMessageId: string,
): WebviewThinkingBlock | undefined {
  return [...session.timeline]
    .reverse()
    .find(
      (item): item is WebviewThinkingBlock =>
        item.type === "thinking" &&
        item.assistantMessageId === assistantMessageId,
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

  const existing = findThinkingByAssistantMessageId(
    session,
    assistantMessageId,
  );
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
  const assistantIndex = findTimelineIndex(
    session,
    assistantMessageId,
    "message",
  );
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
        item.type === "tool" &&
        item.toolCallId === toolCallId &&
        !!item.assistantMessageId,
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
): WebviewThinkingBlock | undefined {
  const assistantMessageId =
    findAssistantGroupIdForToolCallIds(session, options.toolCallIds ?? []) ??
    (typeof options.assistantMessageId === "string" &&
    options.assistantMessageId.length > 0
      ? options.assistantMessageId
      : undefined) ??
    runtime.activeAssistantId ??
    [...session.timeline]
      .reverse()
      .find(
        (item): item is WebviewThinkingBlock =>
          item.type === "thinking" && !!item.assistantMessageId,
      )?.assistantMessageId;
  if (!assistantMessageId) {
    return undefined;
  }
  const thinking = ensureThinkingBlockForAssistantMessage(
    session,
    runtime,
    assistantMessageId,
  );
  thinking.summaryTitle = summaryTitle;
  return thinking;
}

function upsertTool(
  session: WebviewSessionSnapshot,
  toolCallId: string,
  toolName: string,
): WebviewToolCard {
  const existing = session.timeline.find(
    (item): item is WebviewToolCard =>
      item.type === "tool" && item.toolCallId === toolCallId,
  );
  if (existing) {
    return existing;
  }
  const next: WebviewToolCard = {
    id: toolCallId,
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
): WebviewToolCard | undefined {
  const tool = session.timeline.find(
    (item): item is WebviewToolCard =>
      item.type === "tool" && item.toolCallId === toolCallId,
  );
  if (tool) {
    tool.summaryTitle = summaryTitle;
  }
  return tool;
}

function upsertApproval(
  session: WebviewSessionSnapshot,
  request: AskQuestionWireRequest,
  sessionId?: string | null,
  live = false,
): WebviewApprovalCard {
  const identity = approvalIdentity(request);
  const existing = session.timeline.find(
    (item): item is WebviewApprovalCard =>
      item.type === "approval" && approvalIdentity(item.request) === identity,
  );
  if (existing) {
    // A history rebuild may only supply a `[pending]` placeholder. Never let
    // it replace a live control request's answerable connection-scoped route.
    if (live || !existing.live) {
      existing.request = request;
      existing.sessionId = sessionId;
    }
    existing.live = existing.live || live;
    existing.resolved = false;
    return existing;
  }
  const created: WebviewApprovalCard = {
    // A durable toolCallId identifies the question across reconnects, but is
    // also the live tool card id. Namespace this render/patch key so both
    // timeline items can coexist.
    id: `approval:${identity}`,
    live,
    request,
    resolved: false,
    sessionId,
    type: "approval",
  };
  session.timeline.push(created);
  return created;
}

/// `toolCallId` survives a restart whereas `requestId` belongs to one host connection.
/// A historical placeholder and its re-armed control request therefore must address one card.
function approvalIdentity(request: AskQuestionWireRequest): string {
  return request.toolCallId?.trim() || `request:${request.requestId}`;
}

function pendingApprovalRequest(
  sessionId: string,
  toolCallId: string,
  args: Record<string, unknown> | undefined,
): AskQuestionWireRequest | null {
  if (!Array.isArray(args?.questions)) {
    return null;
  }
  return {
    // A persisted `[pending]` result proves the question exists, but cannot
    // be answered until serve re-arms it with a connection-scoped request id.
    requestId: "",
    responseEvent: "",
    sessionId,
    toolCallId,
    questions: args.questions as AskQuestionWireRequest["questions"],
  };
}

function resolveApprovalByToolCallId(
  session: WebviewSessionSnapshot,
  toolCallId: string,
): void {
  for (const item of session.timeline) {
    if (item.type === "approval" && item.request.toolCallId === toolCallId) {
      item.resolved = true;
    }
  }
}

function pushMessage(
  session: WebviewSessionSnapshot,
  kind: WebviewMessageBlock["kind"],
  text: string,
  preferredId?: string | null,
  options: AppendMessageOptions = {},
): WebviewMessageBlock {
  const next: WebviewMessageBlock = {
    id: preferredId ?? createGeneratedTimelineId(session, kind),
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
  if (options.label !== undefined) {
    next.label = options.label;
  }
  if (options.deliveryState) {
    next.deliveryState = options.deliveryState;
  }
  if (options.attachments?.length) {
    next.attachments = options.attachments.map((attachment) => ({
      ...attachment,
    }));
  }
  if (options.retryable !== undefined) {
    next.retryable = options.retryable;
  }
  if (options.recoveryAction !== undefined) {
    next.recoveryAction = options.recoveryAction;
  }
  if (options.recoveryTargetUserMessageId !== undefined) {
    next.recoveryTargetUserMessageId = options.recoveryTargetUserMessageId;
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
        return runtime.localUserMessageIds.has(item.id);
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

function effectiveBusy(
  busy: boolean,
  interrupted: boolean | null | undefined,
): boolean {
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
  return typeof result === "string" && result.trim() === INTERRUPTED_TOOL_RESULT_TEXT;
}

/**
 * GUI-side pending-question predicate. `filterSupersededHistoryEntries` runs before
 * `applyHistoryEntry`, so the result passed here is necessarily the latest active result.
 */
function isPendingAskQuestionResult(
  toolName: string,
  result: unknown,
): boolean {
  return (
    toolName === "ask_question" &&
    typeof result === "string" &&
    result.trim() === PENDING_TOOL_RESULT_TEXT
  );
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
): {
  created: boolean;
  item: WebviewMessageBlock | WebviewThinkingBlock;
} {
  if (kind === "assistant") {
    const current = findTimelineItem(session, assistantMessageId, "message");
    if (current && current.kind === "assistant") {
      current.text += text;
      return {
        created: false,
        item: current,
      };
    }
    const created = pushMessage(session, "assistant", text, assistantMessageId);
    created.assistantMessageId = assistantMessageId;
    runtime.activeAssistantId = assistantMessageId;
    return {
      created: true,
      item: created,
    };
  }

  const current = ensureThinkingBlockForAssistantMessage(
    session,
    runtime,
    assistantMessageId,
  );
  current.text += text;
  return {
    created: current.text === text,
    item: current,
  };
}

function mapSessionToTab(session: SessionSummary): WebviewSessionTab {
  return {
    busy: effectiveBusy(session.busy, session.interrupted),
    isCurrent: session.isCurrent,
    ownedByThisFrontend: true,
    sessionId: session.sessionId,
    title: session.title,
    updatedAt: session.updatedAt,
  };
}

/**
 * Maps an attachment's hash to the URLs a webview can load it from.
 *
 * Injected rather than imported because it needs a live `Webview` and the backend's
 * attachment root, neither of which this store knows about. `unavailable` reports that
 * the hash no longer has bytes behind it.
 */
export type AttachmentUriResolver = (attachment: {
  blobSha: string;
  hasThumb?: boolean;
}) => {
  fullUri: string | null;
  thumbUri: string | null;
  unavailable?: boolean;
};

export class WebviewStateStore {
  private state: WebviewStateSnapshot;
  private attachmentUriResolver: AttachmentUriResolver | null = null;
  private readonly runtimes = new Map<string, SessionRuntimeState>();

  constructor() {
    this.state = {
      activeSessionId: null,
      availableModelCapabilities: {},
      availableModelDetails: {},
      availableModelReasoningLevels: {},
      availableModels: [],
      buildModel: "",
      connectionStatus: "connecting",
      modelAdminSupported: false,
      ready: false,
      sessionViews: {},
      sessions: [],
    };
  }

  private assertUniqueTimelineIds(
    sessions: Iterable<WebviewSessionSnapshot> = Object.values(this.state.sessionViews),
  ): void {
    for (const session of sessions) {
      const ids = new Set<string>();
      for (const item of session.timeline) {
        if (ids.has(item.id)) {
          throw new Error(
            `duplicate timeline id in session ${session.sessionId}: ${item.id}`,
          );
        }
        ids.add(item.id);
      }
    }
  }

  view(): Readonly<WebviewStateSnapshot> {
    return this.state;
  }

  snapshot(): WebviewStateSnapshot {
    return cloneSnapshot(this.state);
  }

  snapshotSession(sessionId: string): WebviewSessionSnapshot | null {
    const session = this.state.sessionViews[sessionId];
    if (!session) {
      return null;
    }
    return cloneSessionSnapshot(session);
  }

  snapshotSessionTab(sessionId: string): WebviewSessionTab | null {
    const tab = this.state.sessions.find((entry) => entry.sessionId === sessionId);
    if (!tab) {
      return null;
    }
    return { ...tab };
  }

  setReady(ready: boolean): void {
    this.state.ready = ready;
    this.state.connectionStatus = ready ? "ready" : "connecting";
  }

  setConnectionStatus(status: WebviewConnectionStatus): void {
    this.state.connectionStatus = status;
    this.state.ready = status === "ready";
  }

  /**
   * Install the hash-to-URL mapping used for history images.
   *
   * Set once the webview exists, and again whenever the attachment root changes, since
   * every URL depends on both. Already-built timelines are re-resolved on the spot so a
   * root that arrives after the first history load does not leave images address-less.
   */
  setAttachmentUriResolver(resolver: AttachmentUriResolver | null): void {
    this.attachmentUriResolver = resolver;
    for (const session of Object.values(this.state.sessionViews)) {
      this.resolveHistoryAttachmentUris(session);
    }
  }

  setAvailableModels(
    models: string[],
    capabilities: Record<string, string[]> = {},
    reasoningLevels: Record<string, string[]> = {},
    modelDetails: WebviewStateSnapshot["availableModelDetails"] = {},
  ): void {
    this.state.availableModelCapabilities = { ...capabilities };
    this.state.availableModelDetails = { ...modelDetails };
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
      availableModelDetails: {},
      availableModelReasoningLevels: {},
      availableModels: [],
      buildModel: "",
      connectionStatus: "connecting",
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
    this.state.sessions = payload.sessions.map((session) =>
      mapSessionToTab(session),
    );
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
    session.agentMode = payload.agentMode ?? "chat";
    session.planTodos = payload.planTodos ?? session.planTodos;
    session.sessionTodos = payload.sessionTodos ?? session.sessionTodos;
    if (payload.contextRatio !== undefined) {
      session.contextRatio = payload.contextRatio ?? null;
    }
    session.activePlan = payload.activePlan
      ? {
        path: payload.activePlan.path,
        planId: payload.activePlan.id,
        state: payload.activePlan.state,
      }
      : null;
    session.ownedByThisFrontend = true;
    this.syncTabOwnedByFrontend(payload.sessionId);
  }

  setComposerDraft(
    sessionId: string,
    draft: NonNullable<WebviewSessionSnapshot["composerDraft"]>,
  ): void {
    this.ensureSession(sessionId).composerDraft = {
      segments: draft.segments.map((segment) => ({ ...segment })),
      text: draft.text,
    };
  }

  setPendingAttachments(
    sessionId: string,
    attachments: WebviewPendingAttachment[],
  ): void {
    this.ensureSession(sessionId).pendingAttachments = [...attachments];
  }

  /**
   * Update every history image that names this hash.
   *
   * Used when a thumbnail is generated after the fact: history views come from the
   * transcript rather than from the draft, so they would otherwise keep showing a
   * placeholder for an image whose thumbnail is already on disk. Matched by hash rather
   * than by attachment id because the same image can appear in several messages.
   */
  updateHistoryAttachments(
    sessionId: string,
    blobSha: string,
    patch: { hasThumb: boolean; thumbUri: string | null },
  ): void {
    for (const item of this.ensureSession(sessionId).timeline) {
      if (item.type !== "message" || !item.attachments) continue;
      item.attachments = item.attachments.map((attachment) =>
        attachment.blobSha === blobSha ? { ...attachment, ...patch } : attachment,
      );
    }
  }

  setCheckpoints(
    sessionId: string,
    checkpoints: SessionCheckpointPayload[],
  ): void {
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
    const session = this.ensureSession(sessionId);
    session.pendingAttachments = [];
    session.composerDraft = {
      segments: [],
      text: "",
    };
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
    runtime.hasMoreHistory =
      history.hasMore === true && typeof history.nextCursor === "string";
    runtime.historyLoading = false;
    this.rebuildHistoryTimeline(sessionId);
  }

  prependHistory(sessionId: string, history: SessionHistoryPayload): void {
    this.prependOlderHistory(sessionId, history);
  }

  prependOlderHistory(sessionId: string, history: SessionHistoryPayload): void {
    const runtime = this.ensureRuntime(sessionId);
    runtime.historyEntries = mergeHistoryEntries(
      history.messages,
      runtime.historyEntries,
    );
    runtime.oldestHistoryCursor = history.nextCursor ?? null;
    runtime.hasMoreHistory =
      history.hasMore === true && typeof history.nextCursor === "string";
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
    pushMessage(
      this.ensureSession(sessionId),
      kind,
      text,
      options.preferredId,
      options,
    );
  }

  appendLocalUserMessage(
    sessionId: string,
    text: string,
    options: {
      attachments?: NonNullable<WebviewMessageBlock["attachments"]>;
      messageId: string;
      segments?: WebviewMessageSegment[];
      submitKind: UserSubmitKind;
    },
  ): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    this.dropOtherFailedLocalUserMessages(sessionId, options.messageId);
    pushMessage(session, "user", text, options.messageId, {
      deliveryState: "pending",
      attachments: options.attachments,
      segments: options.segments,
      submitKind: options.submitKind,
    });
    runtime.localUserMessageIds.add(options.messageId);
  }

  dismissErrorRecovery(sessionId: string, errorId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const card = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "error" && item.id === errorId,
    );
    if (card) {
      runtime.dismissedErrorCards.set(errorId, { ...card });
      card.recoveryAction = undefined;
    }
    runtime.dismissedErrorIds.add(errorId);
  }

  restoreDismissedErrorRecovery(sessionId: string, errorId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    if (!runtime.dismissedErrorIds.delete(errorId)) {
      return;
    }
    this.rebuildHistoryTimeline(sessionId);
    if (
      !session.timeline.some(
        (item) => item.type === "message" && item.kind === "error" && item.id === errorId,
      )
    ) {
      const card = runtime.dismissedErrorCards.get(errorId);
      if (card) {
        upsertTimelineItem(session, card);
      }
    }
    runtime.dismissedErrorCards.delete(errorId);
  }

  setErrorRecoveryRejection(sessionId: string, errorId: string, reason: string): void {
    const session = this.ensureSession(sessionId);
    const card = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "error" && item.id === errorId,
    );
    if (card) {
      card.recoveryError = reason;
    }
  }

  markLocalUserMessageFailed(
    sessionId: string,
    messageId: string,
    error: string,
    retryable: boolean,
    detail?: string,
  ): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" &&
        item.kind === "user" &&
        item.id === messageId,
    );
    if (!message) {
      return;
    }
    message.deliveryError = error;
    message.deliveryErrorDetail = detail ?? error;
    message.deliveryState = "failed";
    message.retryable = retryable;
    runtime.localUserMessageIds.add(messageId);
  }

  markLocalUserMessagePending(sessionId: string, messageId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" &&
        item.kind === "user" &&
        item.id === messageId,
    );
    if (!message) {
      return;
    }
    delete message.deliveryError;
    delete message.deliveryErrorDetail;
    message.deliveryState = "pending";
    delete message.retryable;
    runtime.localUserMessageIds.add(messageId);
  }

  markLocalUserMessageConfirmed(sessionId: string, messageId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    this.dropOtherFailedLocalUserMessages(sessionId, messageId);
    runtime.localUserMessageIds.delete(messageId);
    const message = session.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" &&
        item.kind === "user" &&
        item.id === messageId,
    );
    if (!message) {
      return;
    }
    delete message.deliveryState;
    delete message.deliveryError;
    delete message.deliveryErrorDetail;
    delete message.retryable;
  }

  resolveApproval(requestId: string, result?: AskQuestionResult): void {
    for (const session of Object.values(this.state.sessionViews)) {
      for (const item of session.timeline) {
        if (item.type === "approval" && item.request.requestId === requestId) {
          item.resolved = true;
          if (result) {
            const toolCallId = item.request.toolCallId ?? item.request.requestId;
            upsertTimelineItem(session, {
              args: { questions: item.request.questions },
              id: `ask-question-result-${toolCallId}`,
              isError: false,
              status: "complete",
              summary: JSON.stringify(result),
              toolCallId,
              toolName: "ask_question",
              type: "tool",
            });
          }
        }
      }
    }
  }

  applyEvent(frame: HostEventFrameContent): SessionRenderMutation {
    const mutation = this.applyEventUnchecked(frame);
    this.assertUniqueTimelineIds();
    return mutation;
  }

  private applyEventUnchecked(frame: HostEventFrameContent): SessionRenderMutation {
    if (
      frame.type === "__test.capture_dom" ||
      frame.type === "__test.dom_action" ||
      frame.type === "contextSearchResult" ||
      frame.type === "pathsResolved" ||
      frame.type === "insertReference" ||
      frame.type === "attachFilesResult" ||
      frame.type === "attachmentFeedback" ||
      frame.type === "composerWorkResult" ||
      frame.type === "draftForkResult" ||
      frame.type === "captureDraftForFork" ||
      frame.type === "preview.ready" ||
      frame.type === "preview.select" ||
      frame.type === "preview.save" ||
      frame.type === "preview.close"
    ) {
      return NO_SESSION_RENDER_MUTATION;
    }
    if ("subtype" in frame && frame.type === "control_request") {
      return this.applyControlRequest(frame);
    }

    const session = this.ensureSession(
      frame.sessionId ?? this.state.activeSessionId ?? "unknown",
    );
    const runtime = this.ensureRuntime(session.sessionId);
    switch (frame.type) {
      case "turn_start":
        runtime.turnHadAssistantText = false;
        clearStreaming(runtime);
        return NO_SESSION_RENDER_MUTATION;
      case "message_start": {
        clearStreaming(runtime);
        if (
          "assistantMessageId" in frame &&
          typeof frame.assistantMessageId === "string"
        ) {
          runtime.activeAssistantId = frame.assistantMessageId;
          runtime.streamingAssistantId = frame.assistantMessageId;
        }
        return NO_SESSION_RENDER_MUTATION;
      }
      case "message_end":
        if (
          !("assistantMessageId" in frame) ||
          typeof frame.assistantMessageId !== "string" ||
          runtime.streamingAssistantId === frame.assistantMessageId
        ) {
          clearStreaming(runtime);
        }
        return NO_SESSION_RENDER_MUTATION;
      case "turn_end": {
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (summaryTitle) {
          applySummaryTitleToGroup(session, runtime, summaryTitle, {
            assistantMessageId:
              "assistantMessageId" in frame &&
              typeof frame.assistantMessageId === "string"
                ? frame.assistantMessageId
                : undefined,
            toolCallIds:
              "toolCallIds" in frame && Array.isArray(frame.toolCallIds)
                ? frame.toolCallIds.filter(
                    (toolCallId): toolCallId is string =>
                      typeof toolCallId === "string",
                  )
                : [],
          });
        }
        abortRunningCodeReviewRows(session);
        clearActiveAssistant(runtime);
        return sessionRenderMutation(session.sessionId);
      }
      case "agent_start":
        session.busy = true;
        this.syncTabBusy(session.sessionId, true);
        clearActiveAssistant(runtime);
        return sessionRenderMutation(session.sessionId);
      case "agent_end":
        clearActiveAssistant(runtime);
        {
          const recovery = currentTurnRecoveryAction(session);
          const recoveryOptions = {
            recoveryAction: recovery?.action,
      recoveryError: null,
            recoveryTargetUserMessageId: recovery?.targetUserMessageId,
          };
        if (frame.error && frame.error !== "interrupted") {
          pushMessage(session, "error", frame.error, undefined, recoveryOptions);
        } else if (!frame.error && !runtime.turnHadAssistantText) {
          pushMessage(
            session,
            "notice",
            "本轮没有产生可见回答。",
          );
        }
        }
        return sessionRenderMutation(session.sessionId);
      case "agent_interrupted":
        clearActiveAssistant(runtime);
        markRunningToolsInterrupted(session);
        abortRunningCodeReviewRows(session);
        if (!messageExistsAtTail(session, "warn", "Tomcat turn interrupted")) {
          pushMessage(session, "warn", "Tomcat turn interrupted");
        }
        return sessionRenderMutation(session.sessionId);
      case "agent_idle":
        session.busy = false;
        this.syncTabBusy(session.sessionId, false);
        settleRunningTools(session);
        clearActiveAssistant(runtime);
        return sessionRenderMutation(session.sessionId);
      case "llm_notice":
        pushMessage(session, "notice", frame.message);
        return sessionRenderMutation(session.sessionId);
      case "llm_error":
        {
          const display = formatLlmErrorDisplay({
            errorMessage: frame.errorMessage,
            reason: frame.reason,
          });
          pushMessage(session, "error", display.text, undefined, {
            label: display.label ?? undefined,
          });
        }
        return sessionRenderMutation(session.sessionId);
      case "extension_error":
        pushMessage(session, "error", `${frame.event}: ${frame.error}`);
        return sessionRenderMutation(session.sessionId);
      case "context_metrics_update":
        if (
          frame.providerUsageMeasured ||
          typeof session.contextRatio === "number"
        ) {
          session.contextRatio = frame.contextUtilizationRatio;
        }
        return sessionRenderMutation(session.sessionId);
      case "compaction_error":
        pushMessage(
          session,
          "notice",
          `Context compaction failed: ${frame.error}`,
        );
        return sessionRenderMutation(session.sessionId);
      case "auto_retry_start":
        pushMessage(
          session,
          "notice",
          `Retrying after error: ${frame.errorMessage}`,
        );
        return sessionRenderMutation(session.sessionId);
      case "auto_retry_end":
        if (!frame.success) {
          pushMessage(
            session,
            "notice",
            `Retry finished without success: ${frame.finalError ?? "unknown error"}`,
          );
        }
        return sessionRenderMutation(session.sessionId);
      case "sub_agent_start":
        if (frame.subagentType !== "code_reviewer") {
          pushMessage(
            session,
            "notice",
            `Started ${frame.subagentType} sub-agent`,
          );
        }
        return sessionRenderMutation(session.sessionId);
      case "sub_agent_end":
        if (frame.subagentType !== "code_reviewer") {
          pushMessage(
            session,
            "notice",
            `Sub-agent ${frame.subagentType} ${frame.outcome}`,
          );
        }
        return sessionRenderMutation(session.sessionId);
      case "message_update": {
        const delta = getAssistantDelta(frame);
        if (!delta) {
          return NO_SESSION_RENDER_MUTATION;
        }
        const assistantMessageId =
          "assistantMessageId" in frame &&
          typeof frame.assistantMessageId === "string"
            ? frame.assistantMessageId
            : null;
        if (!assistantMessageId) {
          return NO_SESSION_RENDER_MUTATION;
        }
        if (!runtime.streamingAssistantId && !runtime.activeAssistantId) {
          runtime.activeAssistantId = assistantMessageId;
          runtime.streamingAssistantId = assistantMessageId;
        }
        if (runtime.streamingAssistantId !== assistantMessageId) {
          return NO_SESSION_RENDER_MUTATION;
        }
        if (delta.kind === "content_delta") {
          if (delta.delta.trim().length > 0) {
            runtime.turnHadAssistantText = true;
          }
          runtime.activeAssistantId = assistantMessageId;
          const next = appendStreamingMessage(
            session,
            runtime,
            "assistant",
            assistantMessageId,
            delta.delta,
          );
          if (next.created) {
            const op = buildUpsertPatchOpForItem(session, next.item);
            return patchRenderMutation(
              session.sessionId,
              op ? [op] : [],
            );
          }
          return patchRenderMutation(session.sessionId, [
            {
              id: next.item.id,
              text: delta.delta,
              type: "appendText",
            },
          ]);
        }
        if (delta.kind === "thinking_delta") {
          runtime.activeAssistantId = assistantMessageId;
          const next = appendStreamingMessage(
            session,
            runtime,
            "thinking",
            assistantMessageId,
            delta.delta,
          );
          if (next.created) {
            const op = buildUpsertPatchOpForItem(session, next.item);
            return patchRenderMutation(
              session.sessionId,
              op ? [op] : [],
            );
          }
          return patchRenderMutation(session.sessionId, [
            {
              id: next.item.id,
              text: delta.delta,
              type: "appendText",
            },
          ]);
        }
        return NO_SESSION_RENDER_MUTATION;
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
        const op = buildUpsertPatchOpForItem(session, tool);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
      }
      case "tool_call_streaming":
      case "tool_execution_update": {
        clearThinkingStreaming(runtime);
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        if (!(tool.backgroundRunning && tool.status === "complete")) {
          tool.status = "streaming";
        }
        if ("args" in frame) {
          tool.args = parseToolArgs(frame.args) ?? tool.args;
        }
        if (frame.type === "tool_execution_update") {
          applyLiveToolOutput(tool, frame.partialResult);
        }
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        applyPlanReference(tool);
        const op = buildUpsertPatchOpForItem(session, tool);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
      }
      case "tool_execution_end": {
        clearThinkingStreaming(runtime);
        const activeAssistantId = runtime.activeAssistantId ?? undefined;
        const tool = upsertTool(session, frame.toolCallId, frame.toolName);
        applyToolDisplay(tool, frame.display);
        tool.isError = frame.isError;
        tool.status = toolResultWasInterrupted(frame.result)
          ? "interrupted"
          : "complete";
        tool.summary = toolResultWasInterrupted(frame.result)
          ? "Interrupted"
          : asText(frame.result);
        tool.assistantMessageId = activeAssistantId ?? tool.assistantMessageId;
        applyPlanReference(tool, tool.summary);
        applyBackgroundTaskTicket(tool, tool.summary);
        if (!tool.isError && tool.status === "complete") {
          tool.planActivity = derivePlanActivity(
            tool.toolName,
            tool.summary,
            tool.args,
          );
        } else {
          delete tool.planActivity;
        }
        const op = buildUpsertPatchOpForItem(session, tool);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
      }
      case "plan.todos": {
        const todos = parseTodos("todos" in frame ? frame.todos : undefined);
        session.planTodos = todos;
        if ("planId" in frame && typeof frame.planId === "string") {
          if (session.activePlan) {
            session.activePlan = {
              ...session.activePlan,
              planId: frame.planId,
            };
          }
        }
        return sessionRenderMutation(session.sessionId);
      }
      case "session.todos":
        session.sessionTodos = parseTodos(
          "todos" in frame ? frame.todos : undefined,
        );
        return sessionRenderMutation(session.sessionId);
      case "session.title_updated": {
        const title =
          "title" in frame && typeof frame.title === "string"
            ? frame.title
            : null;
        if (!title) {
          return NO_SESSION_RENDER_MUTATION;
        }
        const tab = this.state.sessions.find(
          (entry) => entry.sessionId === session.sessionId,
        );
        if (!tab) {
          this.syncTabOwnedByFrontend(session.sessionId);
        }
        const nextTab = this.state.sessions.find(
          (entry) => entry.sessionId === session.sessionId,
        );
        if (nextTab) {
          nextTab.title = title;
        }
        return sessionRenderMutation(session.sessionId);
      }
      case "turn.summary_updated": {
        const summaryTitle =
          "summaryTitle" in frame && typeof frame.summaryTitle === "string"
            ? frame.summaryTitle
            : null;
        if (!summaryTitle) {
          return NO_SESSION_RENDER_MUTATION;
        }
        const thinking = applySummaryTitleToGroup(session, runtime, summaryTitle, {
          assistantMessageId:
            "assistantMessageId" in frame &&
            typeof frame.assistantMessageId === "string"
              ? frame.assistantMessageId
              : undefined,
          toolCallIds:
            "toolCallIds" in frame && Array.isArray(frame.toolCallIds)
              ? frame.toolCallIds.filter(
                  (toolCallId): toolCallId is string =>
                    typeof toolCallId === "string",
                )
              : [],
        });
        const op = buildUpsertPatchOpForItem(session, thinking);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
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
          return NO_SESSION_RENDER_MUTATION;
        }
        const tool = applyToolSummaryTitle(session, toolCallId, summaryTitle);
        const op = buildUpsertPatchOpForItem(session, tool);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
      }
      case "background_task_finished": {
        const taskId =
          "taskId" in frame && typeof frame.taskId === "string"
            ? frame.taskId
            : null;
        if (!taskId) {
          return NO_SESSION_RENDER_MUTATION;
        }
        const exitCode =
          "exitCode" in frame && typeof frame.exitCode === "number"
            ? frame.exitCode
            : undefined;
        const tool = applyBackgroundTaskFinished(session, taskId, exitCode);
        const op = buildUpsertPatchOpForItem(session, tool);
        return patchRenderMutation(session.sessionId, op ? [op] : []);
      }
      case "session.agent_mode.changed": {
        const agentMode =
          "agentMode" in frame && (frame.agentMode === "chat" || frame.agentMode === "plan")
            ? frame.agentMode
            : null;
        if (!agentMode) {
          return NO_SESSION_RENDER_MUTATION;
        }
        session.agentMode = agentMode;
        return sessionRenderMutation(session.sessionId);
      }
      default:
        if (isPlanEvent(frame as any)) {
          this.applyPlanEvent(session, frame as any);
          return sessionRenderMutation(session.sessionId);
        }
        return NO_SESSION_RENDER_MUTATION;
    }
  }

  private applyControlRequest(frame: ControlRequestFrame): SessionRenderMutation {
    if (frame.subtype !== "ask_question") {
      return NO_SESSION_RENDER_MUTATION;
    }
    const request = parseAskQuestionRequest(frame);
    if (!request) {
      return NO_SESSION_RENDER_MUTATION;
    }
    const session = this.ensureSession(
      frame.sessionId ?? this.state.activeSessionId ?? "unknown",
    );
    upsertApproval(session, request, frame.sessionId, true);
    return sessionRenderMutation(session.sessionId);
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
    const standardToolResultIds = buildHistoryToolResultIds(renderableEntries);
    const branchSummaryTexts = buildBranchSummaryTextLookup(renderableEntries);
    const errorRecoveryActions = buildErrorRecoveryActions(
      renderableEntries,
      session.busy,
      runtime.dismissedErrorIds,
    );
    const historySession = createEmptySession(sessionId);
    for (const entry of renderableEntries) {
      applyHistoryEntry(
        historySession,
        entry,
        historyToolNames,
        toolCallToAssistant,
        historyToolArgs,
        standardToolResultIds,
        errorRecoveryActions,
        branchSummaryTexts,
      );
    }
    const completedAskQuestionToolCalls = new Set(
      historySession.timeline.flatMap((item) =>
        item.type === "tool" && item.toolName === "ask_question"
          ? [item.toolCallId]
          : [],
      ),
    );
    // A second window can retain a live approval card while another window answers it.
    // Once refreshed history contains the real result, close the old card by durable
    // toolCallId (not the connection-scoped requestId) before retaining live items.
    for (const item of session.timeline) {
      if (
        item.type === "approval" &&
        item.request.toolCallId &&
        completedAskQuestionToolCalls.has(item.request.toolCallId)
      ) {
        item.resolved = true;
      }
    }
    const existingKeys = new Set(
      historySession.timeline.map((item) => timelineEntityKey(item)),
    );
    const optimisticTailKeys = collectOptimisticTailKeys(
      session,
      runtime,
      existingKeys,
    );
    const assistantGroupIds = liveAssistantGroupIds(runtime);
    // Approval identity is the durable toolCallId. A live control request owns
    // the only answerable route, so it replaces a historical `[pending]`
    // placeholder in the same chronological slot. This is the one canonical
    // merge rule used after history reconstruction; requestId is never identity.
    for (const live of session.timeline) {
      if (live.type !== "approval" || !live.live) {
        continue;
      }
      const identity = approvalIdentity(live.request);
      const placeholderIndex = historySession.timeline.findIndex(
        (item): item is WebviewApprovalCard =>
          item.type === "approval" &&
          approvalIdentity(item.request) === identity,
      );
      if (placeholderIndex >= 0) {
        historySession.timeline[placeholderIndex] = cloneTimelineItem(live);
        existingKeys.add(timelineEntityKey(live));
      }
    }
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
    // This is the transaction boundary for a history rebuild. A malformed or
    // inconsistent transcript must fail at the write site without replacing the
    // session's currently renderable timeline with a broken candidate.
    this.assertUniqueTimelineIds([historySession]);
    runtime.localUserMessageIds = nextLocalUserMessageIds;
    session.timeline = historySession.timeline;
    if (!session.activePlan && historySession.activePlan) {
      session.activePlan = historySession.activePlan;
    } else if (
      session.activePlan &&
      historySession.activePlan &&
      session.activePlan.path === historySession.activePlan.path
    ) {
      session.activePlan = {
        ...session.activePlan,
        planId:
          session.activePlan.planId ?? historySession.activePlan.planId ?? null,
        state: session.activePlan.state ?? historySession.activePlan.state ?? null,
      };
    }
    if (session.planTodos.length === 0 && historySession.planTodos.length > 0) {
      session.planTodos = historySession.planTodos;
    }
    session.hasMoreHistory = runtime.hasMoreHistory;
    session.historyLoading = runtime.historyLoading;
    this.resolveHistoryAttachmentUris(session);
  }

  /**
   * A session presents only its current attempt. As soon as another prompt starts, older
   * locally failed bubbles leave the optimistic timeline; their durable transcript rows stay
   * intact for diagnostics and are filtered from history once the newer user row is present.
   */
  private dropOtherFailedLocalUserMessages(sessionId: string, exceptMessageId: string): void {
    const session = this.ensureSession(sessionId);
    const runtime = this.ensureRuntime(sessionId);
    const removedMessageIds = new Set<string>();
    session.timeline = session.timeline.filter((item) => {
      const shouldDrop =
        item.type === "message" &&
        item.kind === "user" &&
        item.id !== exceptMessageId &&
        item.deliveryState === "failed";
      if (shouldDrop) {
        removedMessageIds.add(item.id);
      }
      return !shouldDrop;
    });
    for (const messageId of removedMessageIds) {
      runtime.localUserMessageIds.delete(messageId);
    }
  }

  /**
   * Give history images the URLs they need to render.
   *
   * A transcript records a hash and nothing else, so an image rebuilt from history has no
   * address until someone maps it — and a rebuild happens on every session switch and
   * every reopened window. Without this pass those images stay placeholders forever,
   * which looks like "the thumbnail is still loading" and never resolves.
   *
   * The optimistic bubble for a message that was just sent already carries URLs; the
   * resolver produces the same ones from the same hash, so a rebuild is idempotent.
   */
  private resolveHistoryAttachmentUris(session: WebviewSessionSnapshot): void {
    const resolve = this.attachmentUriResolver;
    if (!resolve) return;
    for (const item of session.timeline) {
      if (item.type !== "message" || !item.attachments?.length) continue;
      item.attachments = item.attachments.map((attachment) => ({
        ...attachment,
        ...resolve(attachment),
      }));
    }
  }

  private applyPlanEvent(
    session: WebviewSessionSnapshot,
    event: ServePlanEvent,
  ): void {
    const state = planEventState(event);
    const planId =
      "planId" in event && typeof event.planId === "string" && event.planId.length > 0
        ? event.planId
        : null;
    if (
      "path" in event &&
      typeof event.path === "string" &&
      event.path.length > 0
    ) {
      const nextState = state ?? session.activePlan?.state ?? null;
      syncPlanRef(
        session,
        event.path,
        nextState,
        planId ?? session.activePlan?.planId ?? null,
      );
      stampRunningCreatePlan(
        session,
        event.path,
        planId ?? session.activePlan?.planId ?? null,
      );
    } else if (session.activePlan) {
      session.activePlan = {
        ...session.activePlan,
        planId: planId ?? session.activePlan.planId ?? null,
        state: state ?? session.activePlan.state ?? null,
      };
    }

    switch (event.type) {
      case "plan.review":
        if (event.summary) {
          upsertPlanEventMessage(
            session,
            "notice",
            `Tomcat plan review: ${event.summary}`,
            event.type,
            event.planId,
            event.summary,
          );
        }
        return;
      case "plan.code_review.started":
        if (event.planId) {
          upsertRunningCodeReviewRow(session, {
            planId: event.planId,
            reviewAttemptId: event.reviewAttemptId,
            round: event.round,
            startedAt: Date.now(),
            toolCallId: event.toolCallId,
          });
        }
        return;
      case "plan.code_review":
        if (event.planId) {
          upsertDoneCodeReviewRow(session, {
            aborted: event.aborted,
            findings: event.findings,
            planId: event.planId,
            reviewAttemptId: event.reviewAttemptId,
            round: event.round,
            rounds: event.rounds,
            skipReason: event.skipReason,
            toolCallId: event.toolCallId,
            summary: event.summary,
            verdict: event.verdict,
          });
        }
        return;
      case "plan.verify":
        if (event.verdict) {
          upsertPlanEventMessage(
            session,
            "notice",
            `Tomcat plan verify: ${event.verdict}`,
            event.type,
            event.planId,
            event.verdict,
          );
        }
        return;
      case "plan.review.warning":
      case "plan.code_review.warning":
        {
          const reason = event.reason ?? "review needs attention";
          upsertPlanEventMessage(
            session,
            "warn",
            `Tomcat plan warning: ${reason}`,
            event.type,
            event.planId,
            reason,
          );
        }
        return;
      default:
        return;
    }
  }

  private syncTabOwnedByFrontend(sessionId: string): void {
    const existing = this.state.sessions.find(
      (session) => session.sessionId === sessionId,
    );
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
    const existing = this.state.sessions.find(
      (session) => session.sessionId === sessionId,
    );
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
