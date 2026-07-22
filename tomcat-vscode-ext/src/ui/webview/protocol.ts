import type {
  AskQuestionResult,
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type { ServeAttachment, ServeContentSegment, ServeEvent } from "../../serveClient/wire";
import type { ParticipantPlanState } from "../../shared/planState";

export type WebviewMessageSegment = ServeContentSegment;
export type WebviewReference = Extract<ServeContentSegment, { type: "reference" }>;

export interface ContextSearchMatch {
  description?: string | null;
  reference: WebviewReference;
}

export interface WebviewDomAction {
  kind:
    | "clickTestId"
    | "dragOverTestId"
    | "dragLeaveTestId"
    | "scrollIntoView"
    | "scrollToEdge"
    | "setInputValue"
    | "setRootWidth";
  edge?: "bottom" | "top";
  index?: number;
  scrollBlock?: "center" | "end" | "nearest" | "start";
  testId?: string;
  value?: string;
  widthPx?: number | null;
}

export interface WebviewMessageBlock {
  assistantMessageId?: string;
  detailText?: string | null;
  deliveryError?: string | null;
  deliveryState?: "failed" | "pending";
  id: string;
  kind: "assistant" | "error" | "notice" | "user" | "warn";
  retryable?: boolean;
  segments?: WebviewMessageSegment[];
  submitKind?: "prompt" | "steer";
  text: string;
  type: "message";
}

export interface WebviewThinkingBlock {
  assistantMessageId?: string;
  id: string;
  summaryTitle?: string | null;
  text: string;
  type: "thinking";
}

export interface WebviewBoundaryBlock {
  coveredCount?: number | null;
  id: string;
  summary?: string | null;
  type: "boundary";
}

export interface WebviewCheckpoint {
  changedFiles: string[];
  createdAt: string;
  id: string;
  kind: string;
  label?: string | null;
  messageAnchor?: string | null;
}

export interface WebviewCheckpointMarker {
  changedFiles: string[];
  checkpointId: string;
  createdAt: string;
  id: string;
  kind: string;
  label?: string | null;
  messageAnchor: string;
  type: "checkpoint";
}

export interface WebviewTodo {
  content: string;
  id: string;
  status: "cancelled" | "completed" | "in_progress" | "pending";
}

export type FileDiffTag = "add" | "ctx" | "del";

export interface FileDiffLine {
  newLine?: number | null;
  oldLine?: number | null;
  tag: FileDiffTag;
  text: string;
}

export interface WebviewToolDisplayFile {
  added?: number | null;
  diff?: FileDiffLine[] | null;
  file: string;
  kind: "file";
  removed?: number | null;
}

export interface WebviewToolDisplayPlan {
  kind: "plan";
  plan: string;
}

export interface WebviewToolDisplayText {
  kind: "text";
  text: string;
}

export type WebviewToolDisplay =
  | WebviewToolDisplayFile
  | WebviewToolDisplayPlan
  | WebviewToolDisplayText;

export type WebviewToolStatus = "complete" | "interrupted" | "running" | "streaming";

export interface WebviewToolDiffStat {
  added: number;
  removed: number;
}

export interface WebviewPlanActivity {
  applied?: number;
  checked?: number;
  completed?: number;
  kind: "create" | "update";
  overview?: string | null;
  stateAfter?: ParticipantPlanState | null;
  stateBefore?: ParticipantPlanState | null;
  title?: string | null;
  total?: number;
}

export interface WebviewToolCard {
  args?: Record<string, unknown>;
  assistantMessageId?: string;
  backgroundExitCode?: number;
  backgroundRunning?: boolean;
  backgroundTaskId?: string;
  display?: WebviewToolDisplay;
  diff?: FileDiffLine[];
  diffStat?: WebviewToolDiffStat;
  id: string;
  isError: boolean;
  planActivity?: WebviewPlanActivity;
  planId?: string | null;
  planPath?: string | null;
  startedAt?: number;
  status: WebviewToolStatus;
  summary?: string;
  /** utility-flash 异步生成的命令"目的"短句（bash 卡片标题）；live-only。 */
  summaryTitle?: string | null;
  toolCallId: string;
  toolName: string;
  type: "tool";
}

export interface WebviewPlanFileRef {
  path: string;
  planId?: string | null;
  state: ParticipantPlanState | null;
}

export interface WebviewPlanFileCard extends WebviewPlanFileRef {
  id: string;
  overview?: string;
  title?: string;
  todos?: WebviewTodo[];
  type: "plan";
}

export type WebviewReviewVerdict = "aborted" | "fail" | "partial" | "pass";

export interface WebviewReviewFinding {
  area: string;
  note: string;
  severity: string;
}

export interface WebviewReviewRow {
  findings?: WebviewReviewFinding[];
  id: string;
  planId: string;
  rounds?: number | null;
  status: "done" | "running";
  summary?: string | null;
  type: "review";
  verdict?: WebviewReviewVerdict;
}

export interface WebviewApprovalCard {
  id: string;
  request: AskQuestionWireRequest;
  resolved: boolean;
  sessionId?: string | null;
  type: "approval";
}

export interface WebviewPendingAttachment {
  attachment: ServeAttachment;
  id: string;
  kind: ServeAttachment["kind"];
  label: string;
  mimeType?: string | null;
  path?: string | null;
}

export interface WebviewSessionSnapshot {
  busy: boolean;
  checkpoints?: WebviewCheckpoint[];
  contextRatio?: number | null;
  hasMoreHistory?: boolean;
  historyLoading?: boolean;
  model?: string | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
  thinkingLevel?: string | null;
  ownedByThisFrontend: boolean;
  pendingAttachments: WebviewPendingAttachment[];
  planFile?: WebviewPlanFileRef | null;
  planId?: string | null;
  planState?: ParticipantPlanState | null;
  sessionId: string;
  timeline: WebviewTimelineItem[];
}

export interface WebviewSessionTab {
  busy: boolean;
  isCurrent: boolean;
  ownedByThisFrontend: boolean;
  sessionId: string;
  title: string | null;
  updatedAt: number | null;
}

export interface WebviewStateSnapshot {
  activeSessionId: string | null;
  availableModelCapabilities?: Record<string, string[]>;
  availableModelReasoningLevels?: Record<string, string[]>;
  availableModels: string[];
  buildModel?: string;
  modelAdminSupported: boolean;
  ready: boolean;
  sessionViews: Record<string, WebviewSessionSnapshot>;
  sessions: WebviewSessionTab[];
}

export type WebviewTimelineItem =
  | WebviewApprovalCard
  | WebviewBoundaryBlock
  | WebviewCheckpointMarker
  | WebviewMessageBlock
  | WebviewPlanFileCard
  | WebviewReviewRow
  | WebviewThinkingBlock
  | WebviewToolCard;

export type HostEventFrameContent =
  | ControlRequestFrame
  | ServeEvent
  | {
      matches: ContextSearchMatch[];
      query: string;
      requestId: string;
      sessionId?: string | null;
      truncated: boolean;
      type: "contextSearchResult";
      workspaceAvailable?: boolean;
    }
  | {
      reference: WebviewReference;
      sessionId?: string | null;
      type: "insertReference";
    }
  | {
      type: "__test.capture_dom";
    }
  | {
      action: WebviewDomAction;
      type: "__test.dom_action";
    };

export type HostToWebviewFrame =
  | {
      channel: "event";
      content: HostEventFrameContent;
      done?: boolean;
      messageId: string;
    }
  | {
      channel: "state";
      content: WebviewStateSnapshot;
      messageId: string;
    };

export const THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type WebviewThinkingLevel = (typeof THINKING_LEVELS)[number];

const THINKING_LEVEL_SET = new Set<string>(THINKING_LEVELS);

function isThinkingLevel(value: unknown): value is WebviewThinkingLevel {
  return isString(value) && THINKING_LEVEL_SET.has(value);
}

export type WebviewIntent =
  | {
      messageId: string;
      type: "answerQuestion";
      data: {
        requestId: string;
        result: AskQuestionResult;
      };
    }
  | {
      messageId: string;
      type: "closeSession";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "interrupt";
      data?: {
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "listSessions";
    }
  | {
      messageId: string;
      type: "loadOlderHistory";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "newSession";
      data?: {
        cwd?: string | null;
      };
    }
  | {
      messageId: string;
      type: "prompt" | "steer";
      data: {
        segments?: WebviewMessageSegment[];
        sessionId?: string | null;
        text: string;
        userMessageId?: string;
      };
    }
  | {
      messageId: string;
      type: "pickContext";
      data?: {
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "ready";
    }
  | {
      messageId: string;
      type: "retryUserMessage";
      data: {
        messageId: string;
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "removeAttachment";
      data: {
        attachmentId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "setModel";
      data: {
        modelId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "setBuildModel";
      data: {
        modelId: string;
      };
    }
  | {
      messageId: string;
      type: "setThinkingLevel";
      data: {
        level: WebviewThinkingLevel;
        modelId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "openModelSettings";
      data?: {
        route?: "models" | null;
      };
    }
  | {
      messageId: string;
      type: "setPlanMode";
      data: {
        action: "build" | "enter" | "exit";
        planId?: string | null;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "openFile";
      data: {
        line?: number;
        path: string;
      };
    }
  | {
      messageId: string;
      type: "openDiff";
      data: {
        toolCallId: string;
      };
    }
  | {
      messageId: string;
      type: "resolveDrop";
      data: {
        sessionId?: string | null;
        uris: string[];
      };
    }
  | {
      messageId: string;
      type: "searchContext";
      data: {
        kind?: "file";
        query: string;
        requestId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "showWarningMessage";
      data: {
        message: string;
      };
    }
  | {
      messageId: string;
      type: "listCheckpoints";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "restoreCheckpoint";
      data: {
        checkpointId: string;
        revertFiles: boolean;
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "openPlanFile";
      data: {
        path: string;
      };
    }
  | {
      messageId: string;
      type: "switchSession";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "__test.dom_snapshot";
      data: {
        activeSessionId: string | null;
        approvalCount: number;
        composerControlMetrics: Record<
          string,
          {
            top: number;
            width: number;
          }
        >;
        composerFooterPlanStatus: string | null;
        composerPlanStatusInBarCount: number;
        composerRowCount: number;
        ctxLabel: string | null;
        disabledTestIds: string[];
        expandedThinkingCount: number;
        expandedToolTitles: string[];
        fileChipTopWithinStream: number | null;
        fileChipVisible: boolean;
        historyLoaderVisible: boolean;
        html: string;
        jumpToLatestVisible: boolean;
        planCardTopWithinStream: number | null;
        latestUserTopWithinStream: number | null;
        messageTexts: string[];
        modelDropdownBottom: number | null;
        modelDropdownFullyVisible: boolean;
        modelDropdownHeight: number;
        modelDropdownLeft: number | null;
        modelDropdownRight: number | null;
        modelDropdownTop: number | null;
        overflowAnchor: string | null;
        sessionTabs: string[];
        sessionGroupHeaders: string[];
        sessionMoreButtons: string[];
        stickyPromptText: string | null;
        streamMetrics: {
          clientHeight: number;
          distanceFromBottom: number;
          scrollHeight: number;
          scrollTop: number;
        };
        timelineKinds: string[];
        toolBodyMetrics: Array<{
          clientHeight: number;
          expanded: boolean;
          scrollHeight: number;
          title: string;
        }>;
        toolTitles: string[];
        assistantResponseGroups: number;
        assistantClickablePathCount: number;
        assistantCodeCardCount: number;
        groupFoldTitles: string[];
        userPromptPill: boolean;
        assistantNoCard: boolean;
        planCardCount: number;
        planFooterSameRow: boolean;
        planCardTodoCountText: string | null;
        planCardTitleText: string | null;
        planNoticeReplayed: boolean;
        planStateText: string | null;
        progressRow: boolean;
        loadingShimmerCount: number;
        planTodos: number;
        standaloneThinkingTitles: string[];
        todoWidgetExpanded: boolean;
        todoWidgetItemCount: number;
        todoWidgetTitle: string | null;
        todoWidgetVisible: boolean;
        toolRowFlat: boolean;
        toolRowExpandable: boolean;
        ellipsisAboveGroupHeader: boolean;
        leftGuideLine: boolean;
        toolRowCount: number;
        toolCardCount: number;
        actionToolRowCount: number;
        editDiffBadgeCount: number;
        commandBlockCount: number;
      };
    };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isAskQuestionResultShape(value: unknown): value is AskQuestionResult {
  return (
    isRecord(value) &&
    Array.isArray(value.answers) &&
    typeof value.cancelled === "boolean"
  );
}

function isWebviewReferenceShape(value: unknown): value is WebviewReference {
  return (
    isRecord(value) &&
    value.type === "reference" &&
    (value.kind === "selection" || value.kind === "file") &&
    isString(value.label) &&
    isString(value.path) &&
    (value.lineStart === undefined || value.lineStart === null || typeof value.lineStart === "number") &&
    (value.lineEnd === undefined || value.lineEnd === null || typeof value.lineEnd === "number") &&
    (value.text === undefined || value.text === null || isString(value.text))
  );
}

function isContextSearchMatchShape(value: unknown): value is ContextSearchMatch {
  return (
    isRecord(value) &&
    isWebviewReferenceShape(value.reference) &&
    (value.description === undefined ||
      value.description === null ||
      isString(value.description))
  );
}

export function sanitizeContextSearchMatches(value: unknown): ContextSearchMatch[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter(isContextSearchMatchShape);
}

export function coerceContextSearchResultEvent(
  value: unknown,
): Extract<HostEventFrameContent, { type: "contextSearchResult" }> | null {
  if (
    !isRecord(value) ||
    value.type !== "contextSearchResult" ||
    !isString(value.requestId) ||
    !isString(value.query) ||
    typeof value.truncated !== "boolean"
  ) {
    return null;
  }
  return {
    matches: sanitizeContextSearchMatches(value.matches),
    query: value.query,
    requestId: value.requestId,
    sessionId:
      value.sessionId === undefined || value.sessionId === null || isString(value.sessionId)
        ? value.sessionId
        : undefined,
    truncated: value.truncated,
    type: "contextSearchResult",
    workspaceAvailable:
      value.workspaceAvailable === undefined || typeof value.workspaceAvailable === "boolean"
        ? value.workspaceAvailable
        : undefined,
  };
}

function isWebviewMessageSegmentShape(value: unknown): value is WebviewMessageSegment {
  if (!isRecord(value) || !isString(value.type)) {
    return false;
  }
  if (value.type === "text") {
    return isString(value.text);
  }
  if (value.type === "reference") {
    return isWebviewReferenceShape(value);
  }
  return false;
}

export function isHostToWebviewFrame(value: unknown): value is HostToWebviewFrame {
  return (
    isRecord(value) &&
    isString(value.messageId) &&
    ((value.channel === "state" && isRecord(value.content)) ||
      (value.channel === "event" && isRecord(value.content)))
  );
}

export function isWebviewIntent(value: unknown): value is WebviewIntent {
  if (!isRecord(value) || !isString(value.messageId) || !isString(value.type)) {
    return false;
  }

  switch (value.type) {
    case "ready":
    case "listSessions":
      return true;
    case "pickContext":
      return value.data === undefined || isRecord(value.data);
    case "prompt":
    case "steer":
      return (
        isRecord(value.data) &&
        isString(value.data.text) &&
        (value.data.userMessageId === undefined || isString(value.data.userMessageId)) &&
        (value.data.segments === undefined ||
          (Array.isArray(value.data.segments) &&
            value.data.segments.every(isWebviewMessageSegmentShape)))
      );
    case "interrupt":
      return value.data === undefined || isRecord(value.data);
    case "setModel":
      return isRecord(value.data) && isString(value.data.modelId);
    case "setBuildModel":
      return isRecord(value.data) && isString(value.data.modelId);
    case "setThinkingLevel":
      return (
        isRecord(value.data) &&
        isString(value.data.modelId) &&
        isThinkingLevel(value.data.level)
      );
    case "setPlanMode":
      return (
        isRecord(value.data) &&
        (value.data.action === "build" ||
          value.data.action === "enter" ||
          value.data.action === "exit")
      );
    case "newSession":
      return value.data === undefined || isRecord(value.data);
    case "retryUserMessage":
      return (
        isRecord(value.data) &&
        isString(value.data.messageId) &&
        isString(value.data.sessionId)
      );
    case "loadOlderHistory":
      return isRecord(value.data) && isString(value.data.sessionId);
    case "switchSession":
    case "closeSession":
      return isRecord(value.data) && isString(value.data.sessionId);
    case "openFile":
      return (
        isRecord(value.data) &&
        isString(value.data.path) &&
        (value.data.line === undefined || typeof value.data.line === "number")
      );
    case "openDiff":
      return isRecord(value.data) && isString(value.data.toolCallId);
    case "openPlanFile":
      return isRecord(value.data) && isString(value.data.path);
    case "openModelSettings":
      return (
        value.data === undefined ||
        (isRecord(value.data) &&
          (value.data.route === undefined ||
            value.data.route === null ||
            value.data.route === "models"))
      );
    case "resolveDrop":
      return (
        isRecord(value.data) &&
        Array.isArray(value.data.uris) &&
        value.data.uris.every(isString)
      );
    case "removeAttachment":
      return isRecord(value.data) && isString(value.data.attachmentId);
    case "searchContext":
      return (
        isRecord(value.data) &&
        isString(value.data.requestId) &&
        isString(value.data.query) &&
        (value.data.kind === undefined || value.data.kind === "file") &&
        (value.data.sessionId === undefined ||
          value.data.sessionId === null ||
          isString(value.data.sessionId))
      );
    case "showWarningMessage":
      return isRecord(value.data) && isString(value.data.message);
    case "listCheckpoints":
      return isRecord(value.data) && isString(value.data.sessionId);
    case "restoreCheckpoint":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.checkpointId) &&
        typeof value.data.revertFiles === "boolean"
      );
    case "answerQuestion":
      return (
        isRecord(value.data) &&
        isString(value.data.requestId) &&
        isAskQuestionResultShape(value.data.result)
      );
    case "__test.dom_snapshot":
      return (
        isRecord(value.data) &&
        Array.isArray(value.data.messageTexts) &&
        Array.isArray(value.data.sessionTabs) &&
        Array.isArray(value.data.sessionGroupHeaders) &&
        Array.isArray(value.data.sessionMoreButtons) &&
        Array.isArray(value.data.toolTitles) &&
        typeof value.data.approvalCount === "number" &&
        typeof value.data.html === "string" &&
        typeof value.data.jumpToLatestVisible === "boolean" &&
        (value.data.planCardTopWithinStream === null ||
          typeof value.data.planCardTopWithinStream === "number") &&
        (value.data.latestUserTopWithinStream === null ||
          typeof value.data.latestUserTopWithinStream === "number") &&
        (value.data.overflowAnchor === null || typeof value.data.overflowAnchor === "string") &&
        (value.data.stickyPromptText === null || typeof value.data.stickyPromptText === "string") &&
        typeof value.data.expandedThinkingCount === "number" &&
        typeof value.data.composerRowCount === "number" &&
        Array.isArray(value.data.expandedToolTitles) &&
        Array.isArray(value.data.timelineKinds) &&
        isRecord(value.data.composerControlMetrics) &&
        (value.data.ctxLabel === null || typeof value.data.ctxLabel === "string") &&
        isRecord(value.data.streamMetrics) &&
        typeof value.data.streamMetrics.scrollTop === "number" &&
        typeof value.data.streamMetrics.scrollHeight === "number" &&
        typeof value.data.streamMetrics.clientHeight === "number" &&
        typeof value.data.streamMetrics.distanceFromBottom === "number" &&
        Array.isArray(value.data.toolBodyMetrics) &&
        typeof value.data.assistantResponseGroups === "number" &&
        typeof value.data.assistantClickablePathCount === "number" &&
        typeof value.data.assistantCodeCardCount === "number" &&
        Array.isArray(value.data.groupFoldTitles) &&
        typeof value.data.userPromptPill === "boolean" &&
        typeof value.data.assistantNoCard === "boolean" &&
        typeof value.data.planCardCount === "number" &&
        (value.data.planCardTodoCountText === null ||
          typeof value.data.planCardTodoCountText === "string") &&
        typeof value.data.planNoticeReplayed === "boolean" &&
        (value.data.planStateText === null || typeof value.data.planStateText === "string") &&
        typeof value.data.progressRow === "boolean" &&
        typeof value.data.loadingShimmerCount === "number" &&
        typeof value.data.planTodos === "number" &&
        Array.isArray(value.data.standaloneThinkingTitles) &&
        value.data.standaloneThinkingTitles.every((title) => typeof title === "string") &&
        typeof value.data.todoWidgetExpanded === "boolean" &&
        typeof value.data.todoWidgetItemCount === "number" &&
        (value.data.todoWidgetTitle === null || typeof value.data.todoWidgetTitle === "string") &&
        typeof value.data.todoWidgetVisible === "boolean" &&
        typeof value.data.toolRowFlat === "boolean" &&
        typeof value.data.toolRowExpandable === "boolean" &&
        typeof value.data.ellipsisAboveGroupHeader === "boolean" &&
        typeof value.data.leftGuideLine === "boolean" &&
        typeof value.data.toolRowCount === "number" &&
        typeof value.data.toolCardCount === "number" &&
        typeof value.data.actionToolRowCount === "number" &&
        typeof value.data.editDiffBadgeCount === "number" &&
        typeof value.data.commandBlockCount === "number"
      );
    default:
      return false;
  }
}

export function createHostFrameMessageId(prefix: string): string {
  const random = Math.random().toString(36).slice(2, 10);
  return `${prefix}-${Date.now()}-${random}`;
}

export class PendingMessageTracker<T> {
  private readonly pending = new Map<
    string,
    {
      reject(error: Error): void;
      resolve(value: T): void;
      timeout: NodeJS.Timeout;
    }
  >();

  create(
    messageId: string,
    timeoutMs: number,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(messageId);
        reject(new Error(`Timed out waiting for webview message ${messageId}`));
      }, timeoutMs).unref();
      this.pending.set(messageId, { resolve, reject, timeout });
    });
  }

  resolve(messageId: string, value: T): boolean {
    const pending = this.pending.get(messageId);
    if (!pending) {
      return false;
    }
    clearTimeout(pending.timeout);
    this.pending.delete(messageId);
    pending.resolve(value);
    return true;
  }

  rejectAll(error: Error): void {
    for (const [messageId, pending] of this.pending) {
      clearTimeout(pending.timeout);
      pending.reject(error);
      this.pending.delete(messageId);
    }
  }
}
