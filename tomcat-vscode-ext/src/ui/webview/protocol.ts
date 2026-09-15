import type {
  AskQuestionResult,
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type {
  ServeAttachment,
  ServeContentSegment,
  ServeEvent,
} from "../../serveClient/wire";
import type {
  WebviewAgentMode,
  WebviewPlanFileState,
} from "../../shared/planState";
import type {
  AttachmentCandidate,
  AttachmentResultItem,
} from "../../shared/attachmentProtocol";
import { isSupportedAttachmentMime } from "../../shared/attachmentProtocol";
import {
  parseDraftForkCapture,
  type DraftForkCapture,
  type DraftForkResult,
} from "../../shared/draftForkProtocol";
import type {
  PreviewClose,
  PreviewReady,
  PreviewSave,
  PreviewSelect,
} from "../../shared/imagePreviewProtocol";
import type { PathResolution } from "../../shared/pathResolution";

export type WebviewMessageSegment = ServeContentSegment;
export type WebviewReference = Extract<
  ServeContentSegment,
  { type: "reference" }
>;

export interface ContextSearchMatch {
  description?: string | null;
  reference: WebviewReference;
}

export interface WebviewDomAction {
  kind:
    | "clickTestId"
    | "dragOverTestId"
    | "dragLeaveTestId"
    | "focusTestId"
    | "pasteClipboardFiles"
    | "pressKeyOnTestId"
    | "probeAttachmentFetch"
    | "probeFullResolutionMemory"
    | "probeImagePipeline"
    | "scrollIntoView"
    | "scrollToEdge"
    | "setInputValue"
    | "setRootWidth";
  edge?: "bottom" | "top";
  files?: AttachmentCandidate[];
  index?: number;
  scrollBlock?: "center" | "end" | "nearest" | "start";
  testId?: string;
  value?: string;
  widthPx?: number | null;
}

export interface WebviewMessageBlock {
  abandoned?: boolean;
  assistantMessageId?: string;
  detailText?: string | null;
  failureDomain?: string | null;
  failureKind?: string | null;
  deliveryError?: string | null;
  deliveryErrorDetail?: string | null;
  deliveryState?: "failed" | "pending";
  id: string;
  label?: string | null;
  /**
   * Images attached to this message, as references rather than bytes.
   *
   * The bytes stay in the backend blob store and reach Chromium over the webview
   * resource protocol. Putting them here as base64 was what made a scrolled-back
   * transcript cost hundreds of megabytes: every snapshot pinned another copy on the
   * JavaScript heap.
   */
  attachments?: WebviewAttachmentView[];
  kind: "assistant" | "error" | "notice" | "user" | "warn";
  retryable?: boolean;
  recoveryAction?: "resume" | "retry";
  recoveryError?: string | null;
  /** Durable transcript id of the user message Retry must copy forward. */
  recoveryTargetUserMessageId?: string;
  statusCode?: number | null;
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
  /** Optional title for non-compaction system notes. */
  title?: string | null;
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

export type FileDiffTag = "add" | "ctx" | "del" | "gap";

export interface FileDiffLine {
  newLine?: number | null;
  oldLine?: number | null;
  skippedLines?: number | null;
  tag: FileDiffTag;
  text: string;
}

export interface WebviewToolDisplayFile {
  added?: number | null;
  diff?: FileDiffLine[] | null;
  diffTruncated?: boolean | null;
  expired?: boolean | null;
  file: string;
  kind: "file";
  removed?: number | null;
}

export type WebviewToolDisplayFileStatus = "applied" | "failed" | "skipped";

export interface WebviewToolDisplayFileEntry {
  added?: number | null;
  diff?: FileDiffLine[] | null;
  diffTruncated?: boolean | null;
  expired?: boolean | null;
  file: string;
  note?: string | null;
  range?: string | null;
  removed?: number | null;
  status?: WebviewToolDisplayFileStatus | null;
}

export interface WebviewToolDisplayFiles {
  expired?: boolean | null;
  files: WebviewToolDisplayFileEntry[];
  kind: "files";
  summary: string;
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
  | WebviewToolDisplayFiles
  | WebviewToolDisplayPlan
  | WebviewToolDisplayText;

export type WebviewToolStatus =
  "complete" | "interrupted" | "running" | "streaming";

export interface WebviewToolDiffStat {
  added: number;
  removed: number;
}

export interface WebviewLiveToolOutputPayload {
  kind?: "live_output";
  logPath?: string;
  nextOffset: number;
  output: string;
  sequence: number;
  startOffset: number;
  taskId?: string;
  truncated?: boolean;
}

export interface WebviewPlanActivity {
  applied?: number;
  checked?: number;
  completed?: number;
  kind: "create" | "update";
  overview?: string | null;
  stateAfter?: WebviewPlanFileState | null;
  stateBefore?: WebviewPlanFileState | null;
  title?: string | null;
  total?: number;
}

export interface WebviewToolCard {
  args?: Record<string, unknown>;
  assistantMessageId?: string;
  backgroundExitCode?: number;
  backgroundRunning?: boolean;
  backgroundTaskId?: string;
  liveOutput?: string;
  liveOutputOffset?: number;
  liveOutputSequence?: number;
  liveOutputTruncated?: boolean;
  logPath?: string;
  display?: WebviewToolDisplay;
  diff?: FileDiffLine[];
  diffTruncated?: boolean;
  diffExpired?: boolean;
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
  state: WebviewPlanFileState | null;
}

export interface WebviewPlanFileCard extends WebviewPlanFileRef {
  id: string;
  overview?: string;
  title?: string;
  todos?: WebviewTodo[];
  type: "plan";
}

export type WebviewReviewVerdict =
  | "aborted"
  | "fail"
  | "partial"
  | "pass"
  | "skipped";

export interface WebviewReviewFinding {
  area: string;
  note: string;
  severity: string;
}

export interface WebviewReviewRow {
  anchorToolCallId?: string | null;
  findings?: WebviewReviewFinding[];
  id: string;
  planId: string;
  reviewAttemptId: string;
  round?: number | null;
  rounds?: number | null;
  startedAt?: number | null;
  status: "done" | "running";
  summary?: string | null;
  type: "review";
  verdict?: WebviewReviewVerdict;
}

export interface WebviewApprovalCard {
  id: string;
  /**
   * True only while this frontend owns a currently answerable control request.
   * Historical `[pending]` records are evidence that a question existed, not a
   * valid route for submitting an answer.
   */
  live: boolean;
  request: AskQuestionWireRequest;
  resolved: boolean;
  sessionId?: string | null;
  type: "approval";
}

/**
 * One attachment as the UI sees it: identity, metadata, and two URLs. No bytes.
 *
 * `thumbUri` and `fullUri` are separate on purpose. A 48px square in the attachment
 * strip and a full-screen preview are wildly different memory costs, and pointing both
 * at the same source is how eleven thumbnails came to decode eleven full-size bitmaps.
 */
export interface WebviewAttachmentView {
  /** sha256 of the original bytes; the backend's name for this attachment. */
  blobSha: string;
  /** Original byte count, for display only. */
  bytes?: number;
  filename: string;
  /** Full-resolution URL. Only the preview panel should load this. */
  fullUri?: string | null;
  /** True once a downsampled version exists in the backend. */
  hasThumb?: boolean;
  id: string;
  kind: ServeAttachment["kind"];
  mimeType: string;
  /** Original local path when the host still knows it. */
  path?: string | null;
  /** Downsampled URL, or the full one when no thumbnail exists yet. */
  thumbUri?: string | null;
  /**
   * The backend no longer has these bytes.
   *
   * Applies to history as much as to a draft: a transcript keeps naming an image after
   * its blob has been garbage-collected, and a message that quietly renders an empty
   * square is worse than one that says the image is gone.
   */
  unavailable?: boolean;
}

/**
 * An attachment sitting in the composer, waiting to be sent.
 *
 * Carries the same reference view as history plus the composer-only bits: a label to
 * show, and `unavailable` for the case where the backend no longer has the bytes.
 */
export interface WebviewPendingAttachment extends WebviewAttachmentView {
  label: string;
  path?: string | null;
}

export interface WebviewComposerDraft {
  segments: WebviewMessageSegment[];
  text: string;
}

export interface WebviewSessionSnapshot {
  activePlan?: WebviewPlanFileRef | null;
  agentMode: WebviewAgentMode;
  busy: boolean;
  checkpoints?: WebviewCheckpoint[];
  composerDraft?: WebviewComposerDraft;
  contextRatio?: number | null;
  hasMoreHistory?: boolean;
  historyLoading?: boolean;
  model?: string | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
  thinkingLevel?: string | null;
  ownedByThisFrontend: boolean;
  pendingAttachments: WebviewPendingAttachment[];
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

export interface WebviewMediaRoot {
  fsPath: string;
  webviewBase: string;
}

export interface WebviewModelInfo {
  capabilities: string[];
  contextWindow?: number | null;
  contextWindowOptions: number[];
  description?: string | null;
  id: string;
  modelName?: string | null;
  selectedContextWindow?: number | null;
  selectedReasoningLevel?: string | null;
  supportedReasoningLevels: string[];
}

export type WebviewConnectionStatus =
  | "connecting"
  | "reconnecting"
  | "ready"
  | "degraded"
  | "failed";

export interface WebviewStateSnapshot {
  activeSessionId: string | null;
  availableModelCapabilities?: Record<string, string[]>;
  availableModelDetails?: Record<string, WebviewModelInfo>;
  availableModelReasoningLevels?: Record<string, string[]>;
  availableModels: string[];
  buildModel?: string;
  connectionStatus?: WebviewConnectionStatus;
  mediaRoots?: WebviewMediaRoot[];
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

export type WebviewSessionPatchOp =
  | {
      id: string;
      text: string;
      type: "appendText";
    }
  | {
      afterId?: string | null;
      beforeId?: string | null;
      item: WebviewTimelineItem;
      type: "upsert";
    }
  | {
      id: string;
      type: "remove";
    };

export type HostEventFrameContent =
  | ControlRequestFrame
  | ServeEvent
  | {
      requestId: string;
      results: PathResolution[];
      type: "pathsResolved";
    }
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
      references: WebviewReference[];
      sessionId: string;
      type: "insertReferences";
    }
  | {
      type: "__test.capture_dom";
    }
  | {
      type: "attachFilesResult";
      items: AttachmentResultItem[];
      operationId?: string;
      sessionId?: string;
    }
  | {
      error?: string;
      operationId: string;
      sessionId: string;
      success: boolean;
      type: "composerWorkResult";
    }
  | (DraftForkResult & { type: "draftForkResult" })
  | {
      operationId: string;
      sourceSessionId: string;
      type: "captureDraftForFork";
    }
  | {
      type: "attachmentFeedback";
      data: {
        message: string;
        hasErrors: boolean;
      };
    }
  | {
      accepted: boolean;
      requestId: string;
      sessionId: string;
      type: "answerQuestionResult";
    }
  | PreviewReady
  | PreviewSelect
  | PreviewSave
  | PreviewClose
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
      channel: "sessionPatch";
      content: {
        ops: WebviewSessionPatchOp[];
        seq: number;
        sessionId: string;
      };
      messageId: string;
    }
  | {
      channel: "sessionView";
      content: {
        sessionId: string;
        tab?: WebviewSessionTab | null;
        view: WebviewSessionSnapshot;
      };
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
        sessionId: string;
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
      type: "forkSession";
      data: DraftForkCapture;
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
        operationId?: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "ready";
    }
  | {
      messageId: string;
      type: "webviewError";
      data: {
        message: string;
        stack?: string;
      };
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
      type: "resyncSessionView";
      data: {
        sessionId: string;
      };
    }
  /**
   * A thumbnail the webview generated for an attachment the backend had none for.
   *
   * The only message that carries image bytes besides the paste path, and it carries the
   * small end: a 192px PNG, never the original.
   */
  | {
      messageId: string;
      type: "cacheAttachmentThumbnail";
      data: {
        blobSha: string;
        sessionId: string;
        thumbBase64: string;
      };
    }
  | {
      messageId: string;
      type: "syncComposerDraft";
      data: {
        sessionId: string;
        segments: WebviewMessageSegment[];
        text: string;
      };
    }
  | {
      messageId: string;
      /**
       * Hand pasted or dropped images to the host.
       *
       * The one message in the whole protocol that carries image bytes, and it fires
       * once per paste. Everything afterwards — snapshots, keystrokes, the prompt
       * itself — speaks in hashes.
       *
       * The webview has already downsampled a thumbnail and, for SVG, rendered a PNG,
       * because a webview is the only place in this system with a decoder that can
       * resize during decode rather than after it.
       */
      type: "attachFiles";
      data: {
        operationId?: string;
        sessionId: string;
        files: AttachmentCandidate[];
      };
    }
  | {
      messageId: string;
      type: "openImagePreview";
      data: {
        sessionId: string;
        attachmentId: string;
      };
    }
  | {
      messageId: string;
      type: "removeDraftAttachment";
      data: {
        sessionId: string;
        attachmentId: string;
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
      type: "setContextWindow";
      data: {
        contextWindow: number;
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
      type: "openLink";
      data: {
        href: string;
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
        operationId?: string;
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
      type: "resolvePaths";
      data: {
        paths: string[];
        requestId: string;
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
      type: "compact";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "recoverErrorTurn";
      data: {
        action: "resume" | "retry";
        errorId: string;
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
        answerCardCount: number;
        answerOutcomes: string[];
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
        historyAttachmentThumbCount: number;
        historyAttachmentUnavailableCount: number;
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
        pendingAttachmentStripClientWidth: number;
        pendingAttachmentStripOverflowing: boolean;
        pendingAttachmentStripScrollWidth: number;
        pendingAttachmentThumbCount: number;
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
        reviewRows: Array<{
          anchorToolCallId: string | null;
          id: string;
          status: string;
          top: number;
          verdict: string | null;
        }>;
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
    }
  | {
      // `App` is unmounted after ErrorBoundary catches. The fallback can only
      // truthfully return the rendered document, not App-owned UI metrics.
      messageId: string;
      type: "__test.dom_fallback_snapshot";
      data: {
        html: string;
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
    typeof value.cancelled === "boolean" &&
    (value.outcome === undefined ||
      value.outcome === "answered" ||
      value.outcome === "skipped" ||
      value.outcome === "interrupted" ||
      value.outcome === "host_disconnected" ||
      value.outcome === "cancelled_unknown")
  );
}

function isWebviewReferenceShape(value: unknown): value is WebviewReference {
  return (
    isRecord(value) &&
    value.type === "reference" &&
    (value.kind === "selection" || value.kind === "file") &&
    isString(value.label) &&
    isString(value.path) &&
    (value.lineStart === undefined ||
      value.lineStart === null ||
      typeof value.lineStart === "number") &&
    (value.lineEnd === undefined ||
      value.lineEnd === null ||
      typeof value.lineEnd === "number") &&
    (value.text === undefined || value.text === null || isString(value.text))
  );
}

function isContextSearchMatchShape(
  value: unknown,
): value is ContextSearchMatch {
  return (
    isRecord(value) &&
    isWebviewReferenceShape(value.reference) &&
    (value.description === undefined ||
      value.description === null ||
      isString(value.description))
  );
}

export function sanitizeContextSearchMatches(
  value: unknown,
): ContextSearchMatch[] {
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
      value.sessionId === undefined ||
      value.sessionId === null ||
      isString(value.sessionId)
        ? value.sessionId
        : undefined,
    truncated: value.truncated,
    type: "contextSearchResult",
    workspaceAvailable:
      value.workspaceAvailable === undefined ||
      typeof value.workspaceAvailable === "boolean"
        ? value.workspaceAvailable
        : undefined,
  };
}

function isWebviewMessageSegmentShape(
  value: unknown,
): value is WebviewMessageSegment {
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

export function isHostToWebviewFrame(
  value: unknown,
): value is HostToWebviewFrame {
  return (
    isRecord(value) &&
    isString(value.messageId) &&
    ((value.channel === "state" && isRecord(value.content)) ||
      (value.channel === "event" && isRecord(value.content)) ||
      (value.channel === "sessionView" && isRecord(value.content)) ||
      (value.channel === "sessionPatch" && isRecord(value.content)))
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
    case "webviewError":
      return (
        isRecord(value.data) &&
        isString(value.data.message) &&
        (value.data.stack === undefined || isString(value.data.stack))
      );
    case "pickContext":
      return value.data === undefined || isRecord(value.data);
    case "prompt":
    case "steer":
      return (
        isRecord(value.data) &&
        isString(value.data.text) &&
        (value.data.userMessageId === undefined ||
          isString(value.data.userMessageId)) &&
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
    case "setContextWindow":
      return (
        isRecord(value.data) &&
        isString(value.data.modelId) &&
        typeof value.data.contextWindow === "number" &&
        Number.isInteger(value.data.contextWindow) &&
        value.data.contextWindow > 0
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
    case "forkSession":
      return parseDraftForkCapture(value.data) !== null;
    case "retryUserMessage":
    case "resyncSessionView":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId)
        && (value.type !== "retryUserMessage" || isString(value.data.messageId))
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
    case "openLink":
      return isRecord(value.data) && isString(value.data.href);
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
        (value.data.operationId === undefined || isString(value.data.operationId)) &&
        Array.isArray(value.data.uris) &&
        value.data.uris.every(isString)
      );
    case "cacheAttachmentThumbnail":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.blobSha) &&
        isString(value.data.thumbBase64)
      );
    case "syncComposerDraft":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.text) &&
        Array.isArray(value.data.segments) &&
        value.data.segments.every(isWebviewMessageSegmentShape)
      );
    case "attachFiles":
      return (
        isRecord(value.data) &&
        (value.data.operationId === undefined || isString(value.data.operationId)) &&
        isString(value.data.sessionId) &&
        Array.isArray(value.data.files) &&
        (value.data.files as unknown[]).every(
          (file) =>
            isRecord(file) &&
            isString(file.dataBase64) &&
            isString(file.mimeType) &&
            isSupportedAttachmentMime(file.mimeType),
        )
      );
    case "openImagePreview":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.attachmentId)
      );
    case "removeDraftAttachment":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.attachmentId)
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
    case "resolvePaths":
      return (
        isRecord(value.data) &&
        isString(value.data.requestId) &&
        Array.isArray(value.data.paths) &&
        value.data.paths.every(isString)
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
    case "compact":
      return isRecord(value.data) && isString(value.data.sessionId);
    case "recoverErrorTurn":
      return (
        isRecord(value.data) &&
        isString(value.data.sessionId) &&
        isString(value.data.errorId) &&
        (value.data.action === "retry" || value.data.action === "resume")
      );
    case "answerQuestion":
      return (
        isRecord(value.data) &&
        isString(value.data.requestId) &&
        isString(value.data.sessionId) &&
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
        typeof value.data.answerCardCount === "number" &&
        Array.isArray(value.data.answerOutcomes) &&
        value.data.answerOutcomes.every((outcome) => typeof outcome === "string") &&
        typeof value.data.approvalCount === "number" &&
        typeof value.data.html === "string" &&
        typeof value.data.jumpToLatestVisible === "boolean" &&
        (value.data.planCardTopWithinStream === null ||
          typeof value.data.planCardTopWithinStream === "number") &&
        (value.data.latestUserTopWithinStream === null ||
          typeof value.data.latestUserTopWithinStream === "number") &&
        (value.data.overflowAnchor === null ||
          typeof value.data.overflowAnchor === "string") &&
        (value.data.stickyPromptText === null ||
          typeof value.data.stickyPromptText === "string") &&
        typeof value.data.expandedThinkingCount === "number" &&
        typeof value.data.composerRowCount === "number" &&
        Array.isArray(value.data.expandedToolTitles) &&
        Array.isArray(value.data.timelineKinds) &&
        isRecord(value.data.composerControlMetrics) &&
        (value.data.ctxLabel === null ||
          typeof value.data.ctxLabel === "string") &&
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
        (value.data.planStateText === null ||
          typeof value.data.planStateText === "string") &&
        typeof value.data.progressRow === "boolean" &&
        typeof value.data.loadingShimmerCount === "number" &&
        typeof value.data.planTodos === "number" &&
        Array.isArray(value.data.standaloneThinkingTitles) &&
        value.data.standaloneThinkingTitles.every(
          (title) => typeof title === "string",
        ) &&
        typeof value.data.todoWidgetExpanded === "boolean" &&
        typeof value.data.todoWidgetItemCount === "number" &&
        (value.data.todoWidgetTitle === null ||
          typeof value.data.todoWidgetTitle === "string") &&
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
    case "__test.dom_fallback_snapshot":
      return isRecord(value.data) && isString(value.data.html);
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

  create(messageId: string, timeoutMs: number): Promise<T> {
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
