import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  blobToBase64,
  makeThumbnailFromUrl,
  prepareAttachment,
} from "./attachments/imagePipeline";
import {
  ApprovalCard,
  approvalAnswerKey,
  createApprovalAnswerDraft,
  type ApprovalAnswerDraft,
  type ApprovalAnswerState,
} from "./components/ApprovalCard";
import { AttachmentChips } from "./components/AttachmentChips";
import { AttachmentStrip } from "./components/AttachmentStrip";
import { injectCheckpointMarkers } from "./components/checkpointMarkers";
import { Composer, type ComposerDraft, type ComposerHandle } from "./components/Composer";
import { ImageLightbox, type ZoomedImage } from "./components/ImageLightbox";
import { RestoreConfirmDialog } from "./components/RestoreConfirmDialog";
import { SessionBar } from "./components/SessionBar";
import { StickyUserPrompt } from "./components/StickyUserPrompt";
import { TodoListWidget } from "./components/TodoListWidget";
import { warmRichRenderModules } from "./components/markdown/richRenderRuntime";
import { TranscriptView } from "./components/TranscriptView";
import { readContextSearchDebounceMs } from "./contextSearchConfig";
import { ComposerWorkRegistry } from "./composerWorkRegistry";
import { isWebviewReference, referenceIdentity } from "./contextReferences";
import type {
  AskQuestionResult,
  ContextSearchMatch,
  WebviewAttachmentView,
  HostToWebviewFrame,
  PathResolution,
  VsCodeApiLike,
  WebviewDomAction,
  WebviewMessageBlock,
  WebviewIntent,
  WebviewReference,
  WebviewCheckpoint,
  WebviewComposerDraft,
  WebviewApprovalCard,
  WebviewTimelineItem,
  WebviewStateSnapshot,
} from "./types";
import { reconcileStateSnapshot, mergeSessionViewSnapshot } from "./stateReconcile";
import { applySessionPatchFrame } from "./statePatch";
import { useAutoScroll } from "./useAutoScroll";

const EMPTY_STATE: WebviewStateSnapshot = {
  activeSessionId: null,
  availableModelCapabilities: {},
  availableModelDetails: {},
  availableModelReasoningLevels: {},
  availableModels: [],
  mediaRoots: [],
  modelAdminSupported: false,
  connectionStatus: "connecting",
  ready: false,
  sessionViews: {},
  sessions: [],
};

const MAX_BOOTSTRAP_FILL_REQUESTS = 4;
const TOP_HISTORY_THRESHOLD_PX = 24;
const EMPTY_DRAFT: ComposerDraft = {
  hasContent: false,
  segments: [],
  text: "",
};
const CONTEXT_SEARCH_DEBOUNCE_MS = readContextSearchDebounceMs();
const COMPOSER_DRAFT_DEBOUNCE_MS = 250;
/**
 * How long an attachment error stays on screen.
 *
 * It used to stay forever: nothing ever cleared it, so "image.png: exceeds 4.5 MB" was
 * still sitting under the composer several messages later, describing a paste the user
 * had long since given up on.
 */
const ATTACHMENT_FEEDBACK_TIMEOUT_MS = 8000;

interface PersistedGuiState {
  approvalAnswers: Record<string, ApprovalAnswerState>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readPersistedApprovalAnswers(value: unknown): Record<string, ApprovalAnswerState> {
  if (!isRecord(value) || !isRecord(value.approvalAnswers)) return {};
  const restored: Record<string, ApprovalAnswerState> = {};
  for (const [key, rawState] of Object.entries(value.approvalAnswers)) {
    if (!isRecord(rawState) || !isRecord(rawState.draft)) continue;
    const draft: ApprovalAnswerDraft = {};
    let valid = true;
    for (const [questionId, rawQuestion] of Object.entries(rawState.draft)) {
      if (
        !isRecord(rawQuestion) ||
        typeof rawQuestion.customText !== "string" ||
        (rawQuestion.optionId !== null && typeof rawQuestion.optionId !== "string")
      ) {
        valid = false;
        break;
      }
      draft[questionId] = {
        customText: rawQuestion.customText,
        optionId: rawQuestion.optionId,
      };
    }
    if (valid) {
      restored[key] = { draft, submitting: rawState.submitting === true };
    }
  }
  return restored;
}

function pruneApprovalAnswers(
  snapshot: WebviewStateSnapshot,
  current: Record<string, ApprovalAnswerState>,
): Record<string, ApprovalAnswerState> {
  const unresolved = new Set<string>();
  for (const session of Object.values(snapshot.sessionViews)) {
    for (const item of session.timeline) {
      if (item.type === "approval" && !item.resolved && item.live) {
        unresolved.add(approvalAnswerKey(item.sessionId ?? session.sessionId, item.request.requestId));
      }
    }
  }
  let changed = false;
  const next: Record<string, ApprovalAnswerState> = {};
  for (const [key, answer] of Object.entries(current)) {
    if (unresolved.has(key)) {
      next[key] = answer;
    } else {
      changed = true;
    }
  }
  return changed ? next : current;
}

function persistGuiState(
  vscodeApi: VsCodeApiLike,
  snapshot: WebviewStateSnapshot,
  approvalAnswers: Record<string, ApprovalAnswerState>,
): void {
  const persisted: WebviewStateSnapshot & PersistedGuiState = {
    ...snapshot,
    approvalAnswers,
  };
  vscodeApi.setState?.(persisted);
}

interface ContextSearchState {
  loading: boolean;
  matches: ContextSearchMatch[];
  open: boolean;
  query: string;
  truncated: boolean;
}

const EMPTY_CONTEXT_SEARCH_STATE: ContextSearchState = {
  loading: false,
  matches: [],
  open: false,
  query: "",
  truncated: false,
};

interface PendingComposerSubmission {
  draft: ComposerDraft;
  messageId: string;
  revision: number;
  sessionId: string | null;
}

interface PendingRestoreDialogState {
  changedFiles: string[];
  checkpointId: string;
  draft: ComposerDraft | null;
  originalMessageId: string | null;
  sessionId: string;
}

interface PendingRestoreRefill {
  draft: ComposerDraft;
  originalMessageId: string;
  sessionId: string;
}

type PendingPathResolution = {
  resolve(results: PathResolution[]): void;
};

function createMessageId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function postIntent(
  vscodeApi: VsCodeApiLike,
  type: WebviewIntent["type"],
  data?: Record<string, unknown>,
): void {
  vscodeApi.postMessage({
    data,
    messageId: createMessageId(type),
    type,
  } as WebviewIntent);
}

function draftsEqual(left: ComposerDraft, right: ComposerDraft): boolean {
  return (
    left.hasContent === right.hasContent &&
    left.text === right.text &&
    JSON.stringify(left.segments) === JSON.stringify(right.segments)
  );
}

/** Digest of a hosted draft, used to notice when the host has a different one. */
function composerDraftSignature(draft: WebviewComposerDraft): string {
  return `${draft.text}\u0000${JSON.stringify(draft.segments)}`;
}

/** The draft state the host assumes for a session it has never been told about. */
const EMPTY_COMPOSER_DRAFT: WebviewComposerDraft = { segments: [], text: "" };

function isInsertReferenceEvent(
  content: HostToWebviewFrame["content"],
): content is {
  reference: WebviewReference;
  sessionId: string;
  type: "insertReference";
} {
  return (
    !!content &&
    typeof content === "object" &&
    "type" in content &&
    content.type === "insertReference" &&
    "sessionId" in content &&
    typeof content.sessionId === "string" &&
    "reference" in content &&
    isWebviewReference(content.reference)
  );
}

function isInsertReferencesEvent(
  content: HostToWebviewFrame["content"],
): content is {
  references: WebviewReference[];
  sessionId: string;
  type: "insertReferences";
} {
  return (
    !!content &&
    typeof content === "object" &&
    "type" in content &&
    content.type === "insertReferences" &&
    "sessionId" in content &&
    typeof content.sessionId === "string" &&
    "references" in content &&
    Array.isArray(content.references) &&
    content.references.length > 0 &&
    content.references.every(isWebviewReference)
  );
}

function sanitizeContextSearchMatches(value: unknown): ContextSearchMatch[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    if (!entry || typeof entry !== "object") {
      return [];
    }
    const candidate = entry as Record<string, unknown>;
    if (!isWebviewReference(candidate.reference)) {
      return [];
    }
    if (
      candidate.description !== undefined &&
      candidate.description !== null &&
      typeof candidate.description !== "string"
    ) {
      return [];
    }
    return [{
      description: candidate.description as string | null | undefined,
      reference: candidate.reference,
    }];
  });
}

function parseContextSearchResultEvent(
  content: HostToWebviewFrame["content"],
): {
  matches: ContextSearchMatch[];
  query: string;
  requestId: string;
  sessionId?: string | null;
  truncated: boolean;
  type: "contextSearchResult";
  workspaceAvailable?: boolean;
} | null {
  if (
    !content ||
    typeof content !== "object" ||
    !("type" in content) ||
    content.type !== "contextSearchResult" ||
    !("requestId" in content) ||
    typeof content.requestId !== "string" ||
    !("query" in content) ||
    typeof content.query !== "string" ||
    !("truncated" in content) ||
    typeof content.truncated !== "boolean" ||
    (("workspaceAvailable" in content && content.workspaceAvailable !== undefined) &&
      typeof content.workspaceAvailable !== "boolean")
  ) {
    return null;
  }

  const eventContent = content as {
    query: string;
    requestId: string;
    sessionId?: string | null;
    truncated: boolean;
    workspaceAvailable?: boolean;
  };

  return {
    ...eventContent,
    matches: sanitizeContextSearchMatches((content as { matches?: unknown }).matches),
    type: "contextSearchResult",
  };
}

function parsePathsResolvedEvent(
  content: HostToWebviewFrame["content"],
): { requestId: string; results: PathResolution[] } | null {
  if (
    !content ||
    typeof content !== "object" ||
    !("type" in content) ||
    content.type !== "pathsResolved" ||
    !("requestId" in content) ||
    typeof content.requestId !== "string" ||
    !("results" in content) ||
    !Array.isArray(content.results)
  ) {
    return null;
  }
  const results = content.results.filter(
    (result): result is PathResolution =>
      !!result &&
      typeof result === "object" &&
      "kind" in result &&
      (result.kind === "file" || result.kind === "directory" || result.kind === "missing") &&
      "path" in result &&
      typeof result.path === "string" &&
      "resolvedPath" in result &&
      typeof result.resolvedPath === "string",
  );
  return { requestId: content.requestId, results };
}

function resolvePendingComposerSubmission(
  snapshot: WebviewStateSnapshot,
  pending: PendingComposerSubmission,
): {
  message: WebviewMessageBlock;
  sessionId: string;
} | null {
  const candidateSessionIds = pending.sessionId
    ? [pending.sessionId]
    : Object.keys(snapshot.sessionViews);
  for (const sessionId of candidateSessionIds) {
    const message = snapshot.sessionViews[sessionId]?.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "user" && item.id === pending.messageId,
    );
    if (message) {
      return { message, sessionId };
    }
  }
  return null;
}

function draftTextFromSegments(segments: ComposerDraft["segments"]): string {
  return segments.map((segment) => (segment.type === "text" ? segment.text : segment.label)).join("");
}

function draftFromUserMessage(message: WebviewMessageBlock): ComposerDraft {
  const segments = message.segments?.length
    ? message.segments.map((segment) => ({ ...segment }))
    : [{ text: message.text, type: "text" } as const];
  return {
    hasContent: segments.some(
      (segment) => segment.type === "reference" || segment.text.trim().length > 0,
    ),
    segments,
    text: draftTextFromSegments(segments),
  };
}

function checkpointMarkerById(
  timeline: WebviewTimelineItem[],
  checkpointId: string,
): Extract<WebviewTimelineItem, { type: "checkpoint" }> | null {
  return timeline.find(
    (item): item is Extract<WebviewTimelineItem, { type: "checkpoint" }> =>
      item.type === "checkpoint" && item.checkpointId === checkpointId,
  ) ?? null;
}

function buildRestoreDialogState(
  timeline: WebviewTimelineItem[],
  checkpoints: WebviewCheckpoint[],
  sessionId: string,
  checkpointId: string,
): PendingRestoreDialogState | null {
  const renderedTimeline = injectCheckpointMarkers(timeline, checkpoints);
  const markerIndex = renderedTimeline.findIndex(
    (item) => item.type === "checkpoint" && item.checkpointId === checkpointId,
  );
  if (markerIndex < 0) {
    return null;
  }
  const marker = checkpointMarkerById(renderedTimeline, checkpointId);
  if (!marker) {
    return null;
  }
  const nextUserMessage = renderedTimeline.slice(markerIndex + 1).find(
    (item): item is WebviewMessageBlock => item.type === "message" && item.kind === "user",
  );
  return {
    changedFiles: [...marker.changedFiles],
    checkpointId,
    draft: nextUserMessage ? draftFromUserMessage(nextUserMessage) : null,
    originalMessageId: nextUserMessage?.id ?? null,
    sessionId,
  };
}

function buildDomSnapshot(state: WebviewStateSnapshot) {
  const root = document.getElementById("root");
  const stream = document.querySelector<HTMLElement>('[data-testid="stream-container"]');
  const userMessages = document.querySelectorAll<HTMLElement>('[data-message-kind="user"]');
  const latestUserMessage = userMessages[userMessages.length - 1] ?? null;
  const queryText = (selector: string) =>
    [...document.querySelectorAll(selector)].map((node) => node.textContent ?? "");
  const composerMetricEntries = [
    "attachment-add",
    "mode-select",
    "model-select",
    "thinking-level-select",
    "context-ratio",
    "send-button",
  ]
    .map((testId) => {
      const node = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`);
      if (!node) {
        return null;
      }
      const rect = node.getBoundingClientRect();
      return [
        testId,
        {
          top: rect.top,
          width: rect.width,
        },
      ] as const;
    })
    .filter((entry): entry is readonly [string, { top: number; width: number }] => !!entry);
  const composerControlMetrics = Object.fromEntries(composerMetricEntries);
  const composerBar = document.querySelector<HTMLElement>('[data-testid="composer-bar"]');
  const composerFooterPlanStatus =
    document.querySelector<HTMLElement>('[data-testid="composer-notice-plan"]')?.textContent ??
    null;
  const composerPlanStatusInBarCount = document.querySelectorAll(
    ".tc-composer__bar .tc-notice--plan",
  ).length;
  const stickyPromptText =
    document.querySelector<HTMLElement>('[data-testid="sticky-user-prompt-text"]')?.textContent ?? null;
  const composerRowCount = composerBar
    ? new Set(
        [...composerBar.children]
          .filter((node): node is HTMLElement => node instanceof HTMLElement)
          .map((node) => Math.round(node.getBoundingClientRect().bottom)),
      ).size
    : 0;
  const timelineKinds = [...document.querySelectorAll(".tc-transcript > *")].map((node) => {
    if (!(node instanceof HTMLElement)) {
      return "unknown";
    }
    if (node.dataset.testid === "message-block") {
      return `message:${node.dataset.kind ?? "unknown"}`;
    }
    return node.dataset.testid ?? "unknown";
  });
  const reviewRows = [...document.querySelectorAll<HTMLElement>("[data-review-attempt-id]")].map((row) => {
    const rect = row.getBoundingClientRect();
    return {
      anchorToolCallId: row.dataset.anchorToolCallId ?? null,
      id: row.dataset.reviewAttemptId ?? "",
      status: row.dataset.reviewStatus ?? "",
      top: rect.top,
      verdict: row.dataset.reviewVerdict ?? null,
    };
  });
  const toolBodyMetrics = [...document.querySelectorAll<HTMLElement>('[data-testid="tool-row"]')].map(
    (row) => {
      const title = row.querySelector('[data-testid="tool-row-label"]')?.textContent ?? "";
      const body = row.querySelector<HTMLElement>('[data-testid="tool-row-body"]');
      return {
        clientHeight: body?.clientHeight ?? 0,
        expanded: !!body,
        scrollHeight: body?.scrollHeight ?? 0,
        title,
      };
    },
  );
  const approvalOptionStates = [
    ...document.querySelectorAll<HTMLElement>('[data-testid^="approval-option-"]'),
  ].map((node) => ({
    selected: node.getAttribute("aria-checked") === "true",
    testId: node.dataset.testid ?? "",
  }));
  const approvalInputTestIds = [
    ...document.querySelectorAll<HTMLElement>('[data-testid^="approval-custom-"]'),
  ].map((node) => node.dataset.testid ?? "");
  const disabledTestIds = [
    ...document.querySelectorAll<HTMLElement>("[data-testid]"),
  ]
    .filter((node) => "disabled" in node && Boolean((node as HTMLButtonElement | HTMLInputElement).disabled))
    .map((node) => node.dataset.testid ?? "");
  const transcriptGroups = document.querySelectorAll<HTMLElement>(
    '[data-testid="thinking-group"]',
  );
  const todoWidget = document.querySelector<HTMLElement>('[data-testid="todo-widget"]');
  const todoWidgetList = document.querySelector<HTMLElement>('[data-testid="todo-widget-list"]');
  const todoWidgetTitle =
    document.querySelector<HTMLElement>('[data-testid="todo-widget-title"]')?.textContent ?? null;
  const groupFoldTitles = [
    ...document.querySelectorAll<HTMLElement>('[data-testid="thinking-group-title"]'),
  ].map((node) => node.textContent ?? "");
  const userPillEl = document.querySelector<HTMLElement>(
    '[data-testid="message-block"].tc-message--user',
  );
  const assistantMessageEl = document.querySelector<HTMLElement>(
    '[data-testid="message-block"].tc-message--assistant',
  );
  const toolRowEl = document.querySelector<HTMLElement>('[data-testid="tool-row"]');
  const fileChipEl = document.querySelector<HTMLElement>('[data-testid="file-chip"]');
  const actionToolRows = document.querySelectorAll<HTMLElement>(
    '[data-testid="tool-row"][data-tool-variant="standalone"]',
  );
  const editDiffBadges = document.querySelectorAll('[data-testid="tool-row-diff-badges"]').length;
  const commandBlockCount = document.querySelectorAll(
    '[data-testid="tool-row"][data-tool-category="command"]',
  ).length;
  const pendingAttachmentStrip = document.querySelector<HTMLElement>(
    '[data-attachment-source="draft"]',
  );
  const pendingAttachmentThumbCount = document.querySelectorAll(
    '[data-testid="attachment-thumb"]',
  ).length;
  // Every entry in the draft strip, whatever state it is in: a rendered thumbnail, a
  // placeholder waiting for one, or a chip. Distinct from the thumbnail count because
  // thumbnails arrive a moment after the attachments do.
  const pendingAttachmentItemCount =
    pendingAttachmentStrip?.querySelectorAll(".tc-attachment-strip__item").length ?? 0;
  const pendingPdfChipTitles = [
    ...document.querySelectorAll<HTMLElement>(
      '[data-attachment-source="draft"] .tc-attachment-strip__file-chip',
    ),
  ].map((node) => node.getAttribute("title") ?? "");
  const historyAttachmentThumbCount = document.querySelectorAll(
    '[data-testid="history-attachment-thumb"]',
  ).length;
  const historyPdfChipTitles = [
    ...document.querySelectorAll<HTMLElement>(
      '[data-attachment-source="history"] .tc-attachment-strip__file-chip',
    ),
  ].map((node) => node.getAttribute("title") ?? "");
  /**
   * What the attachment strip actually costs in bitmap memory, measured rather than argued.
   *
   * `naturalWidth`/`naturalHeight` are the dimensions Chromium decoded, so multiplying by
   * 4 bytes per pixel gives the real cost of the decoded image. This is the number the
   * whole reference-based redesign exists to hold down: the same eleven images at full
   * resolution would be four hundred times this.
   */
  const attachmentBitmaps = [
    ...document.querySelectorAll<HTMLImageElement>(".tc-attachment-strip__img"),
  ].map((image) => ({
    height: image.naturalHeight,
    resolution: image.dataset.attachmentResolution ?? null,
    width: image.naturalWidth,
  }));
  const attachmentBitmapBytes = attachmentBitmaps.reduce(
    (total, bitmap) => total + bitmap.width * bitmap.height * 4,
    0,
  );
  const inlineImages = [...document.querySelectorAll<HTMLImageElement>('[data-testid="inline-image"]')].map(
    (image) => ({
      cursor: getComputedStyle(image).cursor,
      naturalHeight: image.naturalHeight,
      naturalWidth: image.naturalWidth,
      src: image.currentSrc || image.src,
    }),
  );
  const blockedInlineImageTexts = [
    ...document.querySelectorAll<HTMLElement>('[data-testid="blocked-inline-image"]'),
  ].map((node) => node.textContent ?? "");
  const lightboxImage = document.querySelector<HTMLImageElement>('[data-testid="image-lightbox-image"]');
  const assistantCodeCardCount = document.querySelectorAll('[data-testid="assistant-code-card"]').length;
  const assistantClickablePathCount = document.querySelectorAll(
    '[data-testid="assistant-clickable-path"]',
  ).length;
  const ctxLabel =
    document.querySelector<HTMLElement>('[data-testid="context-ratio"]')?.textContent ?? null;
  const planCardTodoCountText =
    document.querySelector<HTMLElement>('[data-testid="plan-todos-count"]')?.textContent ?? null;
  const planCardTitleText =
    document.querySelector<HTMLElement>('[data-testid="plan-card-title"]')?.textContent ?? null;
  const latestPlanCard = Array.from(
    document.querySelectorAll<HTMLElement>('[data-testid="plan-card"]'),
  ).at(-1);
  const viewPlanButton = document.querySelector<HTMLElement>('[data-testid="view-plan"]');
  const buildPlanButton = document.querySelector<HTMLElement>('[data-testid="build-plan"]');
  const planFooterSameRow =
    !!viewPlanButton &&
    !!buildPlanButton &&
    Math.abs(
      viewPlanButton.getBoundingClientRect().top +
        viewPlanButton.getBoundingClientRect().height / 2 -
        (buildPlanButton.getBoundingClientRect().top +
          buildPlanButton.getBoundingClientRect().height / 2),
    ) <= 6;
  let ellipsisAboveGroupHeader = false;
  transcriptGroups.forEach((group) => {
    const preamble = group.querySelector<HTMLElement>(".tc-message--assistant");
    const toggle = group.querySelector<HTMLElement>(
      '[data-testid="thinking-group-toggle"]',
    );
    if (preamble && toggle) {
      const position = toggle.compareDocumentPosition(preamble);
      if (position & Node.DOCUMENT_POSITION_PRECEDING) {
        ellipsisAboveGroupHeader = true;
      }
    }
  });
  const streamRect = stream?.getBoundingClientRect();
  const latestUserRect = latestUserMessage?.getBoundingClientRect();
  const latestPlanCardRect = latestPlanCard?.getBoundingClientRect();
  const fileChipRect = fileChipEl?.getBoundingClientRect();
  const modelDropdownRect = document
    .querySelector<HTMLElement>('[data-testid="model-dropdown"]')
    ?.getBoundingClientRect();
  const fileChipTopWithinStream =
    streamRect && fileChipRect ? fileChipRect.top - streamRect.top : null;
  const fileChipVisible =
    !!streamRect &&
    !!fileChipRect &&
    fileChipRect.bottom > streamRect.top &&
    fileChipRect.top < streamRect.bottom;
  const modelDropdownFullyVisible =
    !!modelDropdownRect &&
    modelDropdownRect.height > 0 &&
    modelDropdownRect.top >= 0 &&
    modelDropdownRect.bottom <= window.innerHeight &&
    modelDropdownRect.left >= 0 &&
    modelDropdownRect.right <= window.innerWidth;
  const planNoticeReplayed = queryText('[data-testid="message-text"]').some((text) =>
    text.startsWith("Tomcat plan review:") ||
    text.startsWith("Tomcat plan verify:") ||
    text.startsWith("Tomcat plan warning:"),
  );
  let userPromptPill = false;
  if (userPillEl && streamRect) {
    const pillRect = userPillEl.getBoundingClientRect();
    const leftGap = pillRect.left - streamRect.left;
    const rightGap = streamRect.right - pillRect.right;
    userPromptPill = leftGap > rightGap + 1;
  }
  return {
    activeSessionId: state.activeSessionId,
    answerCardCount: document.querySelectorAll('[data-testid="answer-card"]').length,
    answerOutcomes: [...document.querySelectorAll<HTMLElement>('[data-testid="answer-card"]')]
      .map((card) => card.dataset.outcome ?? ""),
    approvalCount: document.querySelectorAll('[data-testid="approval-card"]').length,
    approvalInputTestIds,
    approvalOptionStates,
    composerControlMetrics,
    composerFooterPlanStatus,
    composerPlanStatusInBarCount,
    composerRowCount,
    ctxLabel,
    disabledTestIds,
    expandedThinkingCount: document.querySelectorAll('[data-testid="thinking-block"] pre').length,
    expandedToolTitles: toolBodyMetrics.filter((entry) => entry.expanded).map((entry) => entry.title),
    fileChipTopWithinStream,
    fileChipVisible,
    historyLoaderVisible: !!document.querySelector('[data-testid="history-loader"]'),
    // Configuration menus are deliberately portaled to document.body so they
    // can escape the dropdown's scroll clipping. The test snapshot must see
    // those sibling layers as well as the React root.
    html: document.body.innerHTML,
    jumpToLatestVisible: !!document.querySelector('[data-testid="scroll-to-bottom"]'),
    planCardTopWithinStream:
      streamRect && latestPlanCardRect ? latestPlanCardRect.top - streamRect.top : null,
    latestUserTopWithinStream:
      streamRect && latestUserRect ? latestUserRect.top - streamRect.top : null,
    messageTexts: queryText('[data-testid="message-text"]'),
    modelDropdownBottom: modelDropdownRect?.bottom ?? null,
    modelDropdownFullyVisible,
    modelDropdownHeight: modelDropdownRect?.height ?? 0,
    modelDropdownLeft: modelDropdownRect?.left ?? null,
    modelDropdownRight: modelDropdownRect?.right ?? null,
    modelDropdownTop: modelDropdownRect?.top ?? null,
    overflowAnchor: stream?.style.overflowAnchor ?? null,
    sessionTabs: queryText('[data-testid="session-option"]'),
    sessionGroupHeaders: queryText('[data-testid="session-group-header"]'),
    sessionMoreButtons: queryText('[data-testid="session-more"]'),
    stickyPromptText,
    streamMetrics: {
      clientHeight: stream?.clientHeight ?? 0,
      distanceFromBottom: stream
        ? Math.max(0, stream.scrollHeight - stream.clientHeight - stream.scrollTop)
        : 0,
      scrollHeight: stream?.scrollHeight ?? 0,
      scrollTop: stream?.scrollTop ?? 0,
    },
    timelineKinds,
    reviewRows,
    toolBodyMetrics,
    toolTitles: queryText('[data-testid="tool-row-label"]'),
    assistantResponseGroups: transcriptGroups.length,
    assistantClickablePathCount,
    assistantCodeCardCount,
    groupFoldTitles,
    userPromptPill,
    assistantNoCard:
      !!assistantMessageEl && !assistantMessageEl.classList.contains("tc-card"),
    planCardCount: document.querySelectorAll('[data-testid="plan-card"]').length,
    planFooterSameRow,
    planCardTodoCountText,
    planCardTitleText,
    planNoticeReplayed,
    planStateText: composerFooterPlanStatus,
    progressRow: !!document.querySelector('[data-testid="progress-row"]'),
    loadingShimmerCount: document.querySelectorAll(".tc-loading-shimmer").length,
    planTodos: document.querySelectorAll('[data-testid^="plan-todo-"]').length,
    standaloneThinkingTitles: queryText('[data-testid="thinking-toggle"] .tc-thinking__title > span:first-child'),
    todoWidgetExpanded: !!todoWidgetList,
    todoWidgetItemCount: document.querySelectorAll('[data-testid="todo-widget-item"]').length,
    todoWidgetTitle,
    todoWidgetVisible: !!todoWidget,
    toolRowFlat: !!toolRowEl && !toolRowEl.closest(".tc-card"),
    toolRowExpandable: !!document.querySelector('[data-testid="tool-row-toggle"]'),
    ellipsisAboveGroupHeader,
    leftGuideLine: !!document.querySelector(".tc-thinking-tool-wrapper"),
    toolRowCount: document.querySelectorAll('[data-testid="tool-row"]').length,
    toolCardCount: document.querySelectorAll('[data-testid="tool-card"]').length,
    actionToolRowCount: actionToolRows.length,
    editDiffBadgeCount: editDiffBadges,
    commandBlockCount,
    historyAttachmentThumbCount,
    historyAttachmentUnavailableCount: document.querySelectorAll(
      '[data-attachment-source="history"] [data-testid="attachment-unavailable"]',
    ).length,
    attachmentBitmapBytes,
    attachmentBitmaps,
    attachmentFetchProbe:
      document.querySelector('[data-testid="attachment-fetch-probe"]')?.textContent ?? null,
    attachmentSkeletonCount: document.querySelectorAll('[data-testid="attachment-skeleton"]')
      .length,
    // Split by strip, because losing the bytes behind a draft attachment and losing them
    // behind a sent one are different situations with different fixes: the draft one is
    // removable, the history one is a record of something that happened.
    attachmentUnavailableCount:
      pendingAttachmentStrip?.querySelectorAll('[data-testid="attachment-unavailable"]')
        .length ?? 0,
    // The composer is a contenteditable rich-text editor, so its text lives in the DOM
    // rather than in a `value` property.
    composerText:
      document.querySelector<HTMLElement>('[data-testid="composer-input"]')?.textContent ??
      null,
    focusedTestId: document.activeElement instanceof HTMLElement
      ? (document.activeElement.dataset.testid ?? null)
      : null,
    fullResolutionProbe:
      document.querySelector('[data-testid="full-resolution-probe"]')?.textContent ?? null,
    imagePipelineProbe:
      document.querySelector('[data-testid="image-pipeline-probe"]')?.textContent ?? null,
    pendingAttachmentStripClientWidth: pendingAttachmentStrip?.clientWidth ?? 0,
    pendingAttachmentStripOverflowing:
      !!pendingAttachmentStrip &&
      pendingAttachmentStrip.scrollWidth > pendingAttachmentStrip.clientWidth,
    pendingAttachmentStripScrollWidth: pendingAttachmentStrip?.scrollWidth ?? 0,
    pendingAttachmentItemCount,
    pendingPdfChipTitles,
    pendingAttachmentThumbCount,
    historyPdfChipTitles,
    inlineImages,
    blockedInlineImageTexts,
    lightboxVisible: !!document.querySelector('[data-testid="image-lightbox"]'),
    lightboxImageNaturalWidth: lightboxImage?.naturalWidth ?? 0,
    lightboxImageSrc: lightboxImage?.currentSrc || lightboxImage?.src || null,
  };
}

/**
 * Exercise the image pipeline inside the real webview and publish the outcome to the DOM.
 *
 * This exists because the two risks in doing pixel work here cannot be reproduced
 * anywhere else: jsdom has neither `createImageBitmap` nor a canvas that can encode,
 * so a unit test proves nothing about whether Chromium in a VS Code webview will
 * actually rasterise an SVG. Both known pitfalls are covered:
 *
 * 1. An SVG with no intrinsic `width`/`height` — `drawImage` has nothing to scale from
 *    and historically fails or draws an empty bitmap.
 * 2. Canvas tainting — if the browser considers the source cross-origin, `toBlob`
 *    throws `SecurityError` and rasterisation is impossible regardless of anything else.
 *
 * The result is written into a hidden node so the existing `captureWebviewDom` bridge
 * can read it back; the harness asserts on it rather than on a screenshot.
 */
async function probeImagePipeline(): Promise<void> {
  const encoder = new TextEncoder();
  const svgWithSize = encoder.encode(
    '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="240"><rect width="400" height="240" fill="#2d7"/></svg>',
  );
  // No width/height, only a viewBox: pitfall 1.
  const svgWithoutSize = encoder.encode(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 240"><circle cx="200" cy="120" r="100" fill="#37d"/></svg>',
  );
  // A design-tool style SVG: inline style, a <style> block and a url(#id) reference.
  // The blacklist this architecture removed used to reject all three.
  const svgDesignTool = encoder.encode(
    [
      '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200">',
      "<style>.a{fill:#e63}</style>",
      '<defs><linearGradient id="g"><stop offset="0" stop-color="#fff"/></linearGradient></defs>',
      '<rect width="320" height="200" fill="url(#g)" style="opacity:.9"/>',
      '<circle class="a" cx="160" cy="100" r="60"/>',
      "</svg>",
    ].join(""),
  );

  const measure = async (base64: string): Promise<{ height: number; width: number }> => {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const size = { height: bitmap.height, width: bitmap.width };
    bitmap.close();
    return size;
  };

  const inspect = async (
    label: string,
    bytes: Uint8Array,
    mimeType: string,
  ): Promise<Record<string, unknown>> => {
    try {
      const prepared = await prepareAttachment({
        bytes: bytes.slice().buffer,
        filename: `${label}.svg`,
        mimeType,
      });
      const thumb = prepared.thumbBase64 ? await measure(prepared.thumbBase64) : null;
      const provider = prepared.providerBase64
        ? await measure(prepared.providerBase64)
        : null;
      return {
        label,
        providerIsPng: prepared.providerMimeType === "image/png",
        providerSize: provider,
        rasterised: !!prepared.providerBase64,
        thumbSize: thumb,
        thumbWithinBudget: thumb ? Math.max(thumb.width, thumb.height) <= 192 : false,
        usedSourceFallback: typeof prepared.providerText === "string",
        warnings: prepared.warnings,
      };
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error), label };
    }
  };

  const results = [
    await inspect("svg-with-size", svgWithSize, "image/svg+xml"),
    await inspect("svg-without-size", svgWithoutSize, "image/svg+xml"),
    await inspect("svg-design-tool", svgDesignTool, "image/svg+xml"),
  ];

  let node = document.querySelector<HTMLElement>('[data-testid="image-pipeline-probe"]');
  if (!node) {
    node = document.createElement("div");
    node.dataset.testid = "image-pipeline-probe";
    node.style.display = "none";
    document.body.appendChild(node);
  }
  node.textContent = JSON.stringify(results);
}

/**
 * Report what the resource protocol actually serves for an attachment URL.
 *
 * Content-addressed files are named by hash alone, and VS Code decides a webview
 * resource's `Content-Type` purely from the file extension
 * (`platform/webview/common/mimeTypes.ts`). A hash has no extension, so this is where we
 * find out whether the browser is being handed something it will render as an image.
 */
async function probeAttachmentFetch(): Promise<void> {
  const image = document.querySelector<HTMLImageElement>(".tc-attachment-strip__img");
  const result: Record<string, unknown> = {
    naturalHeight: image?.naturalHeight ?? null,
    naturalWidth: image?.naturalWidth ?? null,
    src: image?.src ?? null,
  };
  if (image?.src) {
    try {
      const response = await fetch(image.src);
      result.contentType = response.headers.get("content-type");
      result.ok = response.ok;
      result.status = response.status;
      result.bytes = (await response.blob()).size;
    } catch (error) {
      result.error = error instanceof Error ? error.message : String(error);
    }
  }
  let node = document.querySelector<HTMLElement>('[data-testid="attachment-fetch-probe"]');
  if (!node) {
    node = document.createElement("div");
    node.dataset.testid = "attachment-fetch-probe";
    node.style.display = "none";
    document.body.appendChild(node);
  }
  node.textContent = JSON.stringify(result);
}

/**
 * Reproduce the pre-remediation attachment strip and measure what it cost.
 *
 * The old strip pointed its 48px thumbnails at the full-resolution image, so this renders
 * exactly that — the same images, in the same webview, at the same CSS size — and reads
 * back the dimensions Chromium decoded. Without this the "before" number would be a
 * multiplication we did on paper; with it, both sides of the comparison are measurements
 * taken on the same machine minutes apart.
 *
 * The probe images are removed once measured, so the page is left as it was found.
 */
async function probeFullResolutionMemory(rawSources: string | null): Promise<void> {
  let sources: string[] = [];
  try {
    const parsed = JSON.parse(rawSources ?? "[]");
    sources = Array.isArray(parsed) ? parsed.filter((uri) => typeof uri === "string") : [];
  } catch {
    sources = [];
  }

  const stage = document.createElement("div");
  stage.dataset.testid = "full-resolution-stage";
  stage.style.cssText = "position:absolute;left:-9999px;top:0;";
  document.body.appendChild(stage);

  const decoded: Array<{ height: number; width: number }> = [];
  const failed: string[] = [];
  try {
    // Decoded one after another, but all left in the document: eleven simultaneous
    // 48MB decodes is enough to make Chromium start refusing them, which would
    // understate the very cost this probe exists to measure. Sequential decoding still
    // ends with every bitmap resident at once, which is the number we want.
    for (const uri of sources) {
      const image = document.createElement("img");
      image.src = uri;
      image.style.cssText = "width:48px;height:48px;object-fit:cover;";
      stage.appendChild(image);
      try {
        await image.decode();
        decoded.push({ height: image.naturalHeight, width: image.naturalWidth });
      } catch (error) {
        // Expected for the SVG fixture: a hash-named blob is served as an unknown type,
        // which is exactly why SVGs go through a blob URL instead of this path.
        failed.push(error instanceof Error ? error.message : String(error));
      }
    }
  } finally {
    stage.remove();
  }

  let node = document.querySelector<HTMLElement>('[data-testid="full-resolution-probe"]');
  if (!node) {
    node = document.createElement("div");
    node.dataset.testid = "full-resolution-probe";
    node.style.display = "none";
    document.body.appendChild(node);
  }
  node.textContent = JSON.stringify({
    bitmaps: decoded,
    bytes: decoded.reduce((total, size) => total + size.width * size.height * 4, 0),
    failures: failed,
    measured: decoded.length,
    requested: sources.length,
  });
}

function runDomAction(action: WebviewDomAction): void {
  const decodeBase64Bytes = (input: string): Uint8Array<ArrayBuffer> => {
    const decoded = atob(input);
    const bytes = new Uint8Array(new ArrayBuffer(decoded.length));
    for (let index = 0; index < decoded.length; index += 1) {
      bytes[index] = decoded.charCodeAt(index);
    }
    return bytes;
  };
  const buildClipboardFilePayload = (files: NonNullable<WebviewDomAction["files"]>) => {
    const clipboardFiles = files.map((file) => {
      const clipboardFile = new File(
        [decodeBase64Bytes(file.dataBase64)],
        file.filename ?? "attachment.bin",
        {
          type: file.mimeType,
        },
      );
      if (typeof file.sourcePath === "string" && file.sourcePath.length > 0) {
        Object.defineProperty(clipboardFile, "path", {
          configurable: true,
          value: file.sourcePath,
        });
        Object.defineProperty(clipboardFile, "sourcePath", {
          configurable: true,
          value: file.sourcePath,
        });
      }
      return clipboardFile;
    });
    return {
      files: clipboardFiles,
      getData(_format: string) {
        return "";
      },
      items: clipboardFiles.map((file) => ({
        getAsFile() {
          return file;
        },
        kind: "file" as const,
        type: file.type,
      })),
      types: clipboardFiles.map((file) => file.type),
    };
  };
  const dispatchTestComposerValue = (value: string) => {
    window.dispatchEvent(
      new CustomEvent("tomcat:test:set-composer-value", {
        detail: {
          testId: action.testId,
          value,
        },
      }),
    );
  };
  const isEditableElement = (target: HTMLElement | null): target is HTMLElement =>
    target !== null &&
    (target.isContentEditable || target.getAttribute("contenteditable") === "true");
  const createPasteEvent = (clipboardData: Record<string, unknown>): ClipboardEvent => {
    const event = (
      typeof ClipboardEvent === "function"
        ? new ClipboardEvent("paste", {
            bubbles: true,
            cancelable: true,
          })
        : new Event("paste", {
            bubbles: true,
            cancelable: true,
          })
    ) as ClipboardEvent;
    Object.defineProperty(event, "clipboardData", {
      configurable: true,
      value: clipboardData,
    });
    return event;
  };
  const resolveActionTarget = (): HTMLElement | null => {
    const nodes = [...document.querySelectorAll<HTMLElement>(`[data-testid="${action.testId ?? ""}"]`)];
    const resolvedIndex =
      typeof action.index === "number" && action.index < 0
        ? nodes.length + action.index
        : (action.index ?? 0);
    return nodes[resolvedIndex] ?? null;
  };

  if (action.kind === "probeImagePipeline") {
    void probeImagePipeline();
    return;
  }

  if (action.kind === "probeAttachmentFetch") {
    void probeAttachmentFetch();
    return;
  }

  if (action.kind === "probeFullResolutionMemory") {
    void probeFullResolutionMemory(action.value ?? null);
    return;
  }

  if (action.kind === "focusTestId") {
    resolveActionTarget()?.focus();
    return;
  }

  if (action.kind === "pressKeyOnTestId") {
    const target = resolveActionTarget();
    if (!target) {
      return;
    }
    target.focus();
    const key = action.value ?? "Delete";
    target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key }));
    target.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, key }));
    return;
  }

  if (action.kind === "setRootWidth") {
    const root = document.getElementById("root");
    if (!root) {
      return;
    }
    root.style.width =
      typeof action.widthPx === "number" && action.widthPx > 0 ? `${action.widthPx}px` : "";
    window.dispatchEvent(new Event("resize"));
    return;
  }

  if (action.kind === "setInputValue") {
    const target = document.querySelector<HTMLElement>(
      `[data-testid="${action.testId ?? ""}"]`,
    );
    const nextValue = action.value ?? "";
    if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
      const descriptor = Object.getOwnPropertyDescriptor(
        Object.getPrototypeOf(target),
        "value",
      );
      descriptor?.set?.call(target, nextValue);
      target.dispatchEvent(new Event("input", { bubbles: true }));
      target.dispatchEvent(new Event("change", { bubbles: true }));
      return;
    }
    if (isEditableElement(target)) {
      target.focus();
      target.dispatchEvent(
        createPasteEvent({
          getData(format: string) {
            return format === "text/plain" ? nextValue : "";
          },
        }),
      );
      return;
    }
    dispatchTestComposerValue(nextValue);
    return;
  }

  if (action.kind === "pasteClipboardFiles") {
    const target = document.querySelector<HTMLElement>(
      `[data-testid="${action.testId ?? "composer-input"}"]`,
    );
    if (!isEditableElement(target)) {
      return;
    }
    target.focus();
    target.dispatchEvent(createPasteEvent(buildClipboardFilePayload(action.files ?? [])));
    return;
  }

  if (action.kind === "scrollIntoView") {
    const target = resolveActionTarget();
    if (!target) {
      return;
    }
    target.scrollIntoView({
      block: action.scrollBlock ?? "center",
      inline: "nearest",
    });
    window.dispatchEvent(new Event("scroll"));
    return;
  }

  if (action.kind === "clickTestId") {
    const target = resolveActionTarget();
    if (!target) {
      return;
    }
    target.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, view: window }));
    target.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, view: window }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return;
  }

  if (action.kind === "dragOverTestId" || action.kind === "dragLeaveTestId") {
    const target = resolveActionTarget();
    if (!target) {
      return;
    }
    const eventName = action.kind === "dragOverTestId" ? "dragover" : "dragleave";
    const dragEvent = new DragEvent(eventName, {
      bubbles: true,
      cancelable: true,
    });
    target.dispatchEvent(dragEvent);
    return;
  }

  const target = document.querySelector<HTMLElement>(`[data-testid="${action.testId ?? ""}"]`);
  if (!target) {
    return;
  }
  target.scrollTop = action.edge === "top" ? 0 : target.scrollHeight;
  target.dispatchEvent(new Event("scroll", { bubbles: true }));
}

function answerQuestion(
  vscodeApi: VsCodeApiLike,
  sessionId: string,
  requestId: string,
  result: AskQuestionResult,
): void {
  postIntent(vscodeApi, "answerQuestion", {
    requestId,
    result,
    sessionId,
  });
}

function buildContextLabel(contextRatio?: number | null): string {
  if (typeof contextRatio !== "number" || Number.isNaN(contextRatio)) {
    return "";
  }
  return `Ctx ${Math.round(contextRatio * 100)}%`;
}

function currentModeValue(agentMode?: string | null): "chat" | "plan" {
  return agentMode === "plan" ? "plan" : "chat";
}

function submitPrompt(
  vscodeApi: VsCodeApiLike,
  composer: ComposerHandle | null,
  activeSessionId: string | null | undefined,
  canPrompt: boolean,
  onSubmitted: (pending: PendingComposerSubmission) => void,
): void {
  const draft = composer?.getDraft() ?? EMPTY_DRAFT;
  if (!canPrompt || !draft.hasContent) {
    return;
  }
  const userMessageId = createMessageId("user");
  onSubmitted({
    draft,
    messageId: userMessageId,
    revision: 0,
    sessionId: activeSessionId ?? null,
  });
  postIntent(vscodeApi, "prompt", {
    sessionId: activeSessionId ?? null,
    segments: draft.segments,
    text: draft.text,
    userMessageId,
  });
}

export function App({ vscodeApi }: { vscodeApi: VsCodeApiLike }) {
  const [state, setState] = useState<WebviewStateSnapshot>(EMPTY_STATE);
  const [questionAnnouncement, setQuestionAnnouncement] = useState("");
  const [approvalAnswers, setApprovalAnswers] = useState<Record<string, ApprovalAnswerState>>(
    () => readPersistedApprovalAnswers(vscodeApi.getState?.()),
  );
  const [contextSearch, setContextSearch] = useState<ContextSearchState>(
    EMPTY_CONTEXT_SEARCH_STATE,
  );
  const [pendingRestoreDialog, setPendingRestoreDialog] = useState<PendingRestoreDialogState | null>(
    null,
  );
  const [zoomedImage, setZoomedImage] = useState<ZoomedImage | null>(null);
  const [imageAttachmentFeedback, setImageAttachmentFeedback] = useState<{
    hasErrors: boolean;
    message: string;
    /** Bumped on every report so a repeat of the same message still re-announces. */
    seq: number;
  } | null>(null);
  const [draftForkFeedback, setDraftForkFeedback] = useState<{
    error: string | null;
    pending: boolean;
  }>({ error: null, pending: false });
  const pendingDraftForkRef = useRef<{
    clickDraft: ComposerDraft;
    cutoff: number;
    operationId: string | null;
    sourceSessionId: string;
  } | null>(null);
  const attachmentFeedbackSeqRef = useRef(0);
  const stateRef = useRef<WebviewStateSnapshot>(EMPTY_STATE);
  const approvalAnswersRef = useRef(approvalAnswers);
  const composerRef = useRef<ComposerHandle | null>(null);
  const composerWorkRegistryRef = useRef(new ComposerWorkRegistry());
  const pendingInsertionsRef = useRef<Array<{ reference: WebviewReference; sessionId: string }>>([]);
  const pendingComposerSubmissionRef = useRef<PendingComposerSubmission | null>(null);
  const pendingDraftSyncRef = useRef(new Map<string, ComposerDraft>());
  const draftSyncTimerRef = useRef(new Map<string, number>());
  /** What the host was last told, per session, so identical drafts are not resent. */
  const syncedDraftRef = useRef(new Map<string, string>());
  /**
   * The latest real edit for each session. A host snapshot is only a backup: it must not
   * replace a newer local draft while the user is switching between sessions.
   */
  const localComposerDraftsRef = useRef(new Map<string, ComposerDraft>());
  /** Monotonic local edit identity, so returning to identical text is still a new draft. */
  const localDraftRevisionsRef = useRef(new Map<string, number>());
  /** The last host snapshot lets us distinguish a new host reference from a local deletion. */
  const hostComposerDraftsRef = useRef(new Map<string, ComposerDraft>());
  const applyingBackendDraftRef = useRef(false);
  const appliedComposerDraftRef = useRef<{
    // A digest of the draft content rather than a revision counter. The host no longer
    // hands out revisions: the draft lives in the extension layer, and the webview owns
    // it once hydrated, so there is no shared counter left to compare against.
    signature: string;
    sessionId: string;
  } | null>(null);
  const pendingRestoreRefillRef = useRef<PendingRestoreRefill | null>(null);
  const contextSearchRequestSeqRef = useRef(0);
  const latestContextSearchRequestIdRef = useRef<string | null>(null);
  const contextSearchWarningShownRef = useRef(false);
  const pendingPathResolutionsRef = useRef(new Map<string, PendingPathResolution>());
  const expectedPatchSeqBySessionRef = useRef<Record<string, number>>({});
  const sessionPatchResyncPendingRef = useRef<Set<string>>(new Set());
  const streamRef = useRef<HTMLElement | null>(null);
  const transcriptRef = useRef<HTMLElement | null>(null);
  const announcedQuestionsRef = useRef(new Set<string>());
  const previousActiveSessionRef = useRef<string | null>(null);

  const activeSession = useMemo(
    () =>
      state.activeSessionId
        ? state.sessionViews[state.activeSessionId]
        : undefined,
    [state.activeSessionId, state.sessionViews],
  );
  stateRef.current = state;
  approvalAnswersRef.current = approvalAnswers;

  const pendingQuestionCounts = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const [sessionId, session] of Object.entries(state.sessionViews)) {
      counts[sessionId] = session.timeline.filter(
        (item) => item.type === "approval" && !item.resolved && item.live,
      ).length;
    }
    return counts;
  }, [state.sessionViews]);

  useEffect(() => {
    const newlyPending: string[] = [];
    for (const [sessionId, session] of Object.entries(state.sessionViews)) {
      for (const item of session.timeline) {
        if (item.type !== "approval" || item.resolved || !item.live) continue;
        const key = approvalAnswerKey(item.sessionId ?? sessionId, item.request.requestId);
        if (!announcedQuestionsRef.current.has(key)) {
          announcedQuestionsRef.current.add(key);
          newlyPending.push(sessionId);
        }
      }
    }
    if (newlyPending.length > 0) {
      const sessionId = newlyPending[newlyPending.length - 1];
      const sessionTitle =
        state.sessions.find((session) => session.sessionId === sessionId)?.title?.trim()
        || "New session";
      setQuestionAnnouncement(`Question waiting in ${sessionTitle}.`);
    }
  }, [state.sessionViews, state.sessions]);

  useEffect(() => {
    const previous = previousActiveSessionRef.current;
    previousActiveSessionRef.current = state.activeSessionId;
    if (!previous || !state.activeSessionId || previous === state.activeSessionId) return;
    if ((pendingQuestionCounts[state.activeSessionId] ?? 0) === 0) return;
    window.requestAnimationFrame(() => {
      const panel = [...document.querySelectorAll<HTMLElement>("[data-pending-session-id]")]
        .find((candidate) => candidate.dataset.pendingSessionId === state.activeSessionId);
      panel?.querySelector<HTMLElement>("button:not([disabled]), input:not([disabled])")?.focus();
    });
  }, [pendingQuestionCounts, state.activeSessionId]);

  const flushComposerDraft = useCallback((sessionId?: string) => {
    const sessionIds = sessionId
      ? [sessionId]
      : [...pendingDraftSyncRef.current.keys()];
    for (const pendingSessionId of sessionIds) {
      const timer = draftSyncTimerRef.current.get(pendingSessionId);
      if (timer !== undefined) {
        window.clearTimeout(timer);
        draftSyncTimerRef.current.delete(pendingSessionId);
      }
      const draft = pendingDraftSyncRef.current.get(pendingSessionId);
      pendingDraftSyncRef.current.delete(pendingSessionId);
      if (!draft) {
        continue;
      }
      const signature = composerDraftSignature(draft);
      const lastSynced = syncedDraftRef.current.get(pendingSessionId);
      // An empty draft the host was never told about is the state it already assumes;
      // an unchanged draft is nothing to tell it either. Clearing a draft the host *does*
      // hold still syncs, because that is what deletes the file.
      if (signature === (lastSynced ?? composerDraftSignature(EMPTY_COMPOSER_DRAFT))) {
        continue;
      }
      syncedDraftRef.current.set(pendingSessionId, signature);
      postIntent(vscodeApi, "syncComposerDraft", {
        segments: draft.segments,
        sessionId: pendingSessionId,
        text: draft.text,
      });
    }
  }, [vscodeApi]);

  const scheduleComposerDraftSync = useCallback((sessionId: string, draft: ComposerDraft) => {
    pendingDraftSyncRef.current.set(sessionId, draft);
    const previousTimer = draftSyncTimerRef.current.get(sessionId);
    if (previousTimer !== undefined) {
      window.clearTimeout(previousTimer);
    }
    draftSyncTimerRef.current.set(
      sessionId,
      window.setTimeout(() => flushComposerDraft(sessionId), COMPOSER_DRAFT_DEBOUNCE_MS),
    );
  }, [flushComposerDraft]);

  const activeApprovalCount =
    activeSession?.timeline.filter(
      (item) => item.type === "approval" && !item.resolved && item.live,
    ).length ?? 0;
  const activeTimeline = activeSession?.timeline ?? [];
  const oldestTimelineItemId = activeTimeline[0]?.id ?? null;
  const bootstrapFillRef = useRef<{ requestCount: number; sessionId: string | null }>({
    requestCount: 0,
    sessionId: null,
  });
  const topPaginationRef = useRef<{
    active: boolean;
    anchorOldestItemId: string | null;
    sessionId: string | null;
  }>({
    active: false,
    anchorOldestItemId: null,
    sessionId: null,
  });
  const userMessages = activeTimeline.filter(
    (item): item is WebviewMessageBlock => item.type === "message" && item.kind === "user",
  );
  const latestUserMessageId = userMessages.at(-1)?.id ?? null;
  const userMessageCount = userMessages.length;
  const streamContentKey = `${activeSession?.sessionId ?? "none"}:${activeTimeline.length}:${activeApprovalCount}`;
  const canPrompt = state.ready && !activeSession?.busy && !draftForkFeedback.pending;
  const canInterrupt = state.ready;
  const canBuildPlan = state.ready && !!activeSession && !activeSession.busy;
  const modelAdminSupported = state.modelAdminSupported;
  const activeModelCapabilities = activeSession?.model
    ? state.availableModelCapabilities?.[activeSession.model]
    : undefined;
  const activeSessionContextWindow = activeSession?.model
    ? state.availableModelDetails?.[activeSession.model]?.selectedContextWindow
    : undefined;
  const {
    activeStickyMessageId,
    bottomSpacerHeight,
    scrollToLatest,
    userHasScrolled,
  } = useAutoScroll({
    containerRef: streamRef,
    contentRef: transcriptRef,
    contentKey: streamContentKey,
    latestUserMessageId,
    oldestItemKey: oldestTimelineItemId,
    resetKey: activeSession?.sessionId ?? null,
    userMessageCount,
  });
  const stickyUserMessageText =
    userMessages.find((message) => message.id === activeStickyMessageId)?.text ?? null;

  const flushPendingInsertions = () => {
    const activeSessionId = stateRef.current.activeSessionId;
    if (!composerRef.current || !activeSessionId) {
      return;
    }
    const remaining: Array<{ reference: WebviewReference; sessionId: string }> = [];
    for (const insertion of pendingInsertionsRef.current) {
      if (insertion.sessionId !== activeSessionId) {
        remaining.push(insertion);
        continue;
      }
      composerRef.current.insertReference(insertion.reference);
    }
    pendingInsertionsRef.current = remaining;
  };

  const closeMentionFromApp = () => {
    latestContextSearchRequestIdRef.current = null;
    composerRef.current?.closeMention();
  };

  useEffect(() => {
    closeMentionFromApp();
  }, [activeSession?.sessionId]);

  useEffect(() => {
    setPendingRestoreDialog((current) =>
      current && current.sessionId !== (activeSession?.sessionId ?? "") ? null : current,
    );
    setZoomedImage(null);
    if (
      pendingRestoreRefillRef.current &&
      pendingRestoreRefillRef.current.sessionId !== (activeSession?.sessionId ?? "")
    ) {
      pendingRestoreRefillRef.current = null;
    }
  }, [activeSession?.sessionId]);

  useEffect(() => {
    const sessionId = activeSession?.sessionId;
    const composer = composerRef.current;
    if (!sessionId || !composer) {
      return;
    }
    const backendDraft = activeSession.composerDraft;
    const backendComposerDraft: ComposerDraft = {
      hasContent:
        backendDraft?.segments.some(
          (segment) => segment.type === "reference" || segment.text.trim().length > 0,
        ) === true || (backendDraft?.text.trim().length ?? 0) > 0,
      segments: backendDraft?.segments ?? [],
      text: backendDraft?.text ?? "",
    };
    const localDraft = localComposerDraftsRef.current.get(sessionId);
    const previousBackendDraft = hostComposerDraftsRef.current.get(sessionId);
    if (backendDraft) {
      hostComposerDraftsRef.current.set(sessionId, backendComposerDraft);
    }
    let nextDraft = localDraft ?? backendComposerDraft;

    // A received snapshot establishes the baseline used when the user later clears it.
    // A locally edited session remains locally authoritative until send handling retires it.
    if (!localDraft && backendDraft) {
      syncedDraftRef.current.set(sessionId, composerDraftSignature(backendComposerDraft));
    }

    if (localDraft && backendDraft) {
      const localReferenceIds = new Set(
        localDraft.segments.filter(isWebviewReference).map(referenceIdentity),
      );
      const previousHostReferenceIds = new Set(
        previousBackendDraft?.segments.filter(isWebviewReference).map(referenceIdentity),
      );
      const missingReferences = backendDraft.segments
        .filter(isWebviewReference)
        .filter(
          (reference) =>
            !localReferenceIds.has(referenceIdentity(reference))
            && !previousHostReferenceIds.has(referenceIdentity(reference)),
        );
      if (missingReferences.length > 0) {
        applyingBackendDraftRef.current = true;
        try {
          if (!draftsEqual(composer.getDraft(), localDraft)) {
            composer.replaceDraft(localDraft);
          }
          composer.insertReferences(missingReferences);
          nextDraft = composer.getDraft();
          localComposerDraftsRef.current.set(sessionId, nextDraft);
          if (pendingDraftSyncRef.current.has(sessionId)) {
            pendingDraftSyncRef.current.set(sessionId, nextDraft);
          }
        } finally {
          applyingBackendDraftRef.current = false;
        }
      }
    }

    const signature = composerDraftSignature(nextDraft);
    const applied = appliedComposerDraftRef.current;
    if (applied?.sessionId === sessionId && applied.signature === signature) {
      return;
    }
    appliedComposerDraftRef.current = { sessionId, signature };
    if (!draftsEqual(composer.getDraft(), nextDraft)) {
      applyingBackendDraftRef.current = true;
      try {
        composer.replaceDraft(nextDraft);
      } finally {
        applyingBackendDraftRef.current = false;
      }
    }
  }, [activeSession?.composerDraft, activeSession?.sessionId]);

  useEffect(() => {
    return () => {
      flushComposerDraft();
    };
  }, [activeSession?.sessionId, flushComposerDraft]);

  // Attachment feedback expires on its own. Keyed on `seq` so a second identical
  // message restarts the clock instead of inheriting the first one's remaining time.
  useEffect(() => {
    if (!imageAttachmentFeedback) {
      return;
    }
    const timer = window.setTimeout(
      () => setImageAttachmentFeedback(null),
      ATTACHMENT_FEEDBACK_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timer);
  }, [imageAttachmentFeedback?.seq]);

  // Anything that changes what the message was describing clears it early: switching
  // session, or emptying the attachment strip it was talking about.
  useEffect(() => {
    setImageAttachmentFeedback(null);
  }, [activeSession?.sessionId]);

  const pendingAttachmentCount = activeSession?.pendingAttachments.length ?? 0;
  useEffect(() => {
    if (pendingAttachmentCount === 0) {
      setImageAttachmentFeedback(null);
    }
  }, [pendingAttachmentCount]);

  /**
   * Generate the thumbnails the backend does not have yet.
   *
   * Pasting produces a thumbnail as a side effect of reading the clipboard, but the file
   * picker and message history do not: those bytes reach the backend without ever passing
   * through a webview. Attachments in that state have no thumbnail, and the strip refuses
   * to substitute the original — it shows a placeholder instead, because eleven
   * full-resolution decodes is the exact failure this design exists to prevent.
   *
   * So the webview fills the gap itself: read the image over the resource protocol,
   * downsample inside the decoder, hand the small result back. Each hash is attempted
   * once per session; a failure leaves the placeholder rather than retrying forever.
   */
  const attemptedThumbnailsRef = useRef(new Set<string>());
  const thumbnailWorkRef = useRef(false);
  const [thumbnailPass, setThumbnailPass] = useState(0);
  useEffect(() => {
    const session = activeSession;
    if (!session || thumbnailWorkRef.current) return;

    // One at a time, on purpose. Generating eleven at once means eleven simultaneous
    // decodes, which is the memory spike this is here to avoid; and each completed
    // thumbnail changes the snapshot anyway, so a batch would be interrupted mid-flight.
    let next: { blobSha: string; fullUri: string; mimeType: string } | null = null;
    const consider = (attachment: WebviewAttachmentView & { kind?: string }) => {
      if (next || attachment.hasThumb || !attachment.fullUri) return;
      if (attachment.kind === "file" || !attachment.mimeType.startsWith("image/")) return;
      if (attemptedThumbnailsRef.current.has(attachment.blobSha)) return;
      next = {
        blobSha: attachment.blobSha,
        fullUri: attachment.fullUri,
        mimeType: attachment.mimeType,
      };
    };
    for (const attachment of session.pendingAttachments) consider(attachment);
    for (const item of session.timeline) {
      if (item.type !== "message") continue;
      for (const attachment of item.attachments ?? []) consider(attachment);
    }
    if (!next) return;

    const target = next as { blobSha: string; fullUri: string; mimeType: string };
    attemptedThumbnailsRef.current.add(target.blobSha);
    thumbnailWorkRef.current = true;
    void (async () => {
      try {
        const thumb = await makeThumbnailFromUrl(target.fullUri, target.mimeType);
        postIntent(vscodeApi, "cacheAttachmentThumbnail", {
          blobSha: target.blobSha,
          sessionId: session.sessionId,
          thumbBase64: await blobToBase64(thumb),
        });
      } catch (error) {
        // The placeholder stays. Retrying would most likely fail the same way, and a
        // missing thumbnail costs memory rather than correctness.
        console.warn(`Tomcat could not build a thumbnail for ${target.blobSha}`, error);
      } finally {
        thumbnailWorkRef.current = false;
        // Move to the next one. A success also arrives as a new snapshot, but a failure
        // does not, and without this nudge the remaining images would never be attempted.
        setThumbnailPass((pass) => pass + 1);
      }
    })();
  }, [activeSession, thumbnailPass, vscodeApi]);

  useEffect(() => {
    if (!contextSearch.open) {
      return;
    }
    const requestId = `context-search-${++contextSearchRequestSeqRef.current}`;
    latestContextSearchRequestIdRef.current = requestId;
    const timeout = window.setTimeout(() => {
      postIntent(vscodeApi, "searchContext", {
        kind: "file",
        query: contextSearch.query,
        requestId,
        sessionId: activeSession?.sessionId ?? null,
      });
    }, CONTEXT_SEARCH_DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timeout);
    };
  }, [activeSession?.sessionId, contextSearch.open, contextSearch.query, vscodeApi]);

  useEffect(() => {
    const pending = pendingComposerSubmissionRef.current;
    if (!pending) {
      return;
    }
    const resolved = resolvePendingComposerSubmission(state, pending);
    if (!resolved || resolved.message.deliveryState === "pending") {
      return;
    }
    pendingComposerSubmissionRef.current = null;
    if (resolved.message.deliveryState === "failed") {
      return;
    }

    // Confirmation can arrive while another session is on screen. Retire the submitted
    // session's local cache first; otherwise a later switch would resurrect sent input.
    const stored = localComposerDraftsRef.current.get(resolved.sessionId);
    const isSubmittedRevisionCurrent =
      (localDraftRevisionsRef.current.get(resolved.sessionId) ?? 0) === pending.revision;
    if (stored && isSubmittedRevisionCurrent && draftsEqual(stored, pending.draft)) {
      localComposerDraftsRef.current.delete(resolved.sessionId);
      syncedDraftRef.current.delete(resolved.sessionId);
      pendingDraftSyncRef.current.delete(resolved.sessionId);
      const timer = draftSyncTimerRef.current.get(resolved.sessionId);
      if (timer !== undefined) {
        window.clearTimeout(timer);
        draftSyncTimerRef.current.delete(resolved.sessionId);
      }
    }

    if (!isSubmittedRevisionCurrent || state.activeSessionId !== resolved.sessionId) {
      return;
    }
    const composer = composerRef.current;
    if (!composer || !draftsEqual(composer.getDraft(), pending.draft)) {
      return;
    }
    applyingBackendDraftRef.current = true;
    try {
      composer.clear();
    } finally {
      applyingBackendDraftRef.current = false;
    }
  }, [state]);

  useEffect(() => {
    const pending = pendingRestoreRefillRef.current;
    const composer = composerRef.current;
    if (!pending || !composer) {
      return;
    }
    const session = state.sessionViews[pending.sessionId];
    if (!session) {
      pendingRestoreRefillRef.current = null;
      return;
    }
    const originalMessageStillVisible = session.timeline.some(
      (item) => item.type === "message" && item.id === pending.originalMessageId,
    );
    pendingRestoreRefillRef.current = null;
    if (originalMessageStillVisible) {
      return;
    }
    if (state.activeSessionId !== pending.sessionId) {
      return;
    }
    composer.replaceDraft(pending.draft);
  }, [state]);

  useEffect(() => {
    const requestSessionResync = (sessionId: string) => {
      if (sessionPatchResyncPendingRef.current.has(sessionId)) {
        return;
      }
      sessionPatchResyncPendingRef.current.add(sessionId);
      delete expectedPatchSeqBySessionRef.current[sessionId];
      postIntent(vscodeApi, "resyncSessionView", { sessionId });
    };

    const handleMessage = (event: MessageEvent<HostToWebviewFrame>) => {
      const frame = event.data;
      if (!frame || typeof frame !== "object") {
        return;
      }
      if (frame.channel === "state") {
        expectedPatchSeqBySessionRef.current = {};
        sessionPatchResyncPendingRef.current.clear();
        const nextState = reconcileStateSnapshot(stateRef.current, frame.content);
        stateRef.current = nextState;
        const nextAnswers = pruneApprovalAnswers(nextState, approvalAnswersRef.current);
        approvalAnswersRef.current = nextAnswers;
        setApprovalAnswers(nextAnswers);
        setState(nextState);
        persistGuiState(vscodeApi, nextState, nextAnswers);
        flushPendingInsertions();
        return;
      }
      if (frame.channel === "sessionView") {
        delete expectedPatchSeqBySessionRef.current[frame.content.sessionId];
        sessionPatchResyncPendingRef.current.delete(frame.content.sessionId);
        const nextState = mergeSessionViewSnapshot(stateRef.current, frame.content);
        stateRef.current = nextState;
        const nextAnswers = pruneApprovalAnswers(nextState, approvalAnswersRef.current);
        approvalAnswersRef.current = nextAnswers;
        setApprovalAnswers(nextAnswers);
        setState(nextState);
        persistGuiState(vscodeApi, nextState, nextAnswers);
        flushPendingInsertions();
        return;
      }
      if (frame.channel === "sessionPatch") {
        if (sessionPatchResyncPendingRef.current.has(frame.content.sessionId)) {
          return;
        }
        const expectedSeq =
          expectedPatchSeqBySessionRef.current[frame.content.sessionId];
        if (expectedSeq !== undefined && expectedSeq !== frame.content.seq) {
          requestSessionResync(frame.content.sessionId);
          return;
        }
        const patched = applySessionPatchFrame(stateRef.current, frame.content);
        if (!patched.ok) {
          requestSessionResync(frame.content.sessionId);
          return;
        }
        expectedPatchSeqBySessionRef.current[frame.content.sessionId] =
          frame.content.seq + 1;
        stateRef.current = patched.state;
        approvalAnswersRef.current = pruneApprovalAnswers(
          patched.state,
          approvalAnswersRef.current,
        );
        setApprovalAnswers(approvalAnswersRef.current);
        setState(patched.state);
        persistGuiState(vscodeApi, patched.state, approvalAnswersRef.current);
        return;
      }
      if (frame.channel === "event") {
        const insertions = isInsertReferenceEvent(frame.content)
          ? {
              references: [frame.content.reference],
              sessionId: frame.content.sessionId,
            }
          : isInsertReferencesEvent(frame.content)
            ? frame.content
            : null;
        if (insertions) {
          if (composerRef.current && insertions.sessionId === stateRef.current.activeSessionId) {
            composerRef.current.insertReferences(insertions.references);
          } else {
            pendingInsertionsRef.current.push(
              ...insertions.references.map((reference) => ({
                reference,
                sessionId: insertions.sessionId,
              })),
            );
          }
          return;
        }
      }
      if (frame.channel === "event") {
        if (
          typeof frame.content === "object" &&
          frame.content !== null &&
          "type" in frame.content &&
          frame.content.type === "answerQuestionResult" &&
          "accepted" in frame.content &&
          frame.content.accepted === false &&
          "sessionId" in frame.content &&
          typeof frame.content.sessionId === "string" &&
          "requestId" in frame.content &&
          typeof frame.content.requestId === "string"
        ) {
          const key = approvalAnswerKey(frame.content.sessionId, frame.content.requestId);
          const current = approvalAnswersRef.current[key];
          if (current?.submitting) {
            const next = {
              ...approvalAnswersRef.current,
              [key]: { ...current, submitting: false },
            };
            approvalAnswersRef.current = next;
            setApprovalAnswers(next);
            persistGuiState(vscodeApi, stateRef.current, next);
          }
          return;
        }
        if (
          typeof frame.content === "object" &&
          frame.content !== null &&
          "type" in frame.content &&
          frame.content.type === "captureDraftForFork" &&
          "operationId" in frame.content &&
          typeof frame.content.operationId === "string" &&
          "sourceSessionId" in frame.content &&
          typeof frame.content.sourceSessionId === "string"
        ) {
          const operationId = frame.content.operationId;
          const sourceSessionId = frame.content.sourceSessionId;
          let pending = pendingDraftForkRef.current;
          if (pending?.operationId && pending.operationId !== operationId) {
            return;
          }
          if (!pending || pending.sourceSessionId !== sourceSessionId) {
            const draft = composerRef.current?.getDraft() ?? {
              hasContent: false,
              segments: [],
              text: "",
            };
            pending = {
              clickDraft: {
                ...draft,
                segments: draft.segments.map((segment) => ({ ...segment })),
              },
              cutoff: composerWorkRegistryRef.current.cutoff(sourceSessionId),
              operationId,
              sourceSessionId,
            };
            pendingDraftForkRef.current = pending;
          } else {
            pending.operationId = operationId;
          }
          setDraftForkFeedback({ error: null, pending: true });
          const cutoff = pending.cutoff;
          void composerWorkRegistryRef.current.waitForCutoff(sourceSessionId, cutoff).then(() => {
            const current = pendingDraftForkRef.current;
            if (
              !current
              || current.operationId !== operationId
              || current.sourceSessionId !== sourceSessionId
            ) {
              return;
            }
            const sourceStillActive = stateRef.current.activeSessionId === sourceSessionId;
            if (sourceStillActive) flushComposerDraft();
            const draft = sourceStillActive
              ? composerRef.current?.getDraft() ?? current.clickDraft
              : current.clickDraft;
            postIntent(vscodeApi, "forkSession", {
              cwd: null,
              operationId,
              segments: draft.segments,
              sourceSessionId,
              text: draft.text,
            });
          });
          return;
        }
        if (
          typeof frame.content === "object" &&
          frame.content !== null &&
          "type" in frame.content &&
          frame.content.type === "draftForkResult" &&
          "operationId" in frame.content &&
          typeof frame.content.operationId === "string" &&
          "success" in frame.content &&
          typeof frame.content.success === "boolean"
        ) {
          const pending = pendingDraftForkRef.current;
          if (!pending || pending.operationId !== frame.content.operationId) {
            return;
          }
          pendingDraftForkRef.current = null;
          const error =
            !frame.content.success
            && "error" in frame.content
            && typeof frame.content.error === "string"
              ? frame.content.error
              : null;
          setDraftForkFeedback({ error, pending: false });
          return;
        }
        if (
          typeof frame.content === "object" &&
          frame.content !== null &&
          "type" in frame.content &&
          frame.content.type === "composerWorkResult" &&
          "operationId" in frame.content &&
          typeof frame.content.operationId === "string"
        ) {
          composerWorkRegistryRef.current.complete(frame.content.operationId);
          return;
        }
        if (
          typeof frame.content === "object" &&
          frame.content !== null &&
          "type" in frame.content &&
          frame.content.type === "attachmentFeedback" &&
          "data" in frame.content &&
          typeof frame.content.data === "object" &&
          frame.content.data !== null &&
          "message" in frame.content.data &&
          typeof frame.content.data.message === "string" &&
          "hasErrors" in frame.content.data &&
          typeof frame.content.data.hasErrors === "boolean"
        ) {
          setImageAttachmentFeedback({
            hasErrors: frame.content.data.hasErrors,
            message: frame.content.data.message,
            seq: ++attachmentFeedbackSeqRef.current,
          });
          return;
        }
        const pathsResolved = parsePathsResolvedEvent(frame.content);
        if (pathsResolved) {
          const pending = pendingPathResolutionsRef.current.get(pathsResolved.requestId);
          if (!pending) {
            return;
          }
          pendingPathResolutionsRef.current.delete(pathsResolved.requestId);
          pending.resolve(pathsResolved.results);
          return;
        }
        const contextSearchResult = parseContextSearchResultEvent(frame.content);
        if (contextSearchResult) {
          if (contextSearchResult.requestId !== latestContextSearchRequestIdRef.current) {
            return;
          }
          if (contextSearchResult.workspaceAvailable === false) {
            if (!contextSearchWarningShownRef.current) {
              contextSearchWarningShownRef.current = true;
              postIntent(vscodeApi, "showWarningMessage", {
                message: "打开文件夹后可用 @",
              });
            }
            closeMentionFromApp();
            return;
          }
          contextSearchWarningShownRef.current = false;
          setContextSearch((current) => ({
            ...current,
            loading: false,
            matches: contextSearchResult.matches,
            truncated: contextSearchResult.truncated,
          }));
          return;
        }
      }
      if (
        frame.channel === "event" &&
        typeof frame.content === "object" &&
        frame.content !== null &&
        "type" in frame.content &&
        frame.content.type === "__test.capture_dom"
      ) {
        vscodeApi.postMessage({
          data: buildDomSnapshot(stateRef.current),
          messageId: frame.messageId,
          type: "__test.dom_snapshot",
        });
        return;
      }
      if (
        frame.channel === "event" &&
        typeof frame.content === "object" &&
        frame.content !== null &&
        "type" in frame.content &&
        frame.content.type === "__test.dom_action"
      ) {
        runDomAction(frame.content.action as WebviewDomAction);
      }
    };

    window.addEventListener("message", handleMessage);
    postIntent(vscodeApi, "ready");
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [vscodeApi]);

  useEffect(() => {
    flushPendingInsertions();
  }, [state.activeSessionId]);

  useEffect(() => {
    let cancelled = false;
    const scheduleWarmup = () => {
      if (!cancelled) {
        void warmRichRenderModules();
      }
    };
    if (typeof window.requestIdleCallback === "function") {
      const idleId = window.requestIdleCallback(scheduleWarmup);
      return () => {
        cancelled = true;
        window.cancelIdleCallback?.(idleId);
      };
    }
    const timeoutId = window.setTimeout(scheduleWarmup, 0);
    return () => {
      cancelled = true;
      window.clearTimeout(timeoutId);
    };
  }, []);

  const handleApprovalDraftChange = useCallback(
    (sessionId: string, requestId: string, draft: ApprovalAnswerDraft) => {
      const ownerSessionId = sessionId || stateRef.current.activeSessionId;
      if (!ownerSessionId) return;
      const key = approvalAnswerKey(ownerSessionId, requestId);
      const next = {
        ...approvalAnswersRef.current,
        [key]: { draft, submitting: false },
      };
      approvalAnswersRef.current = next;
      setApprovalAnswers(next);
      persistGuiState(vscodeApi, stateRef.current, next);
    },
    [vscodeApi],
  );

  const handleAnswerQuestion = useCallback(
    (sessionId: string, requestId: string, result: AskQuestionResult) => {
      const ownerSessionId = sessionId || stateRef.current.activeSessionId;
      if (!ownerSessionId) return;
      const key = approvalAnswerKey(ownerSessionId, requestId);
      const approval = Object.values(stateRef.current.sessionViews)
        .flatMap((session) => session.timeline)
        .find(
          (item) => item.type === "approval" && item.request.requestId === requestId,
        );
      const current = approvalAnswersRef.current[key] ?? {
        draft:
          approval?.type === "approval"
            ? createApprovalAnswerDraft(approval)
            : {},
        submitting: false,
      };
      if (current.submitting) return;
      const next = {
        ...approvalAnswersRef.current,
        [key]: { ...current, submitting: true },
      };
      approvalAnswersRef.current = next;
      setApprovalAnswers(next);
      persistGuiState(vscodeApi, stateRef.current, next);
      answerQuestion(vscodeApi, ownerSessionId, requestId, result);
    },
    [vscodeApi],
  );
  const handleOpenFile = useCallback(
    (path: string, line?: number) => {
      postIntent(vscodeApi, "openFile", {
        line,
        path,
      });
    },
    [vscodeApi],
  );
  const handleOpenLink = useCallback(
    (href: string) => {
      postIntent(vscodeApi, "openLink", { href });
    },
    [vscodeApi],
  );
  const resolvePaths = useCallback(
    (paths: string[]): Promise<PathResolution[]> => {
      const uniquePaths = [...new Set(paths.map((path) => path.trim()).filter(Boolean))];
      if (uniquePaths.length === 0) {
        return Promise.resolve([]);
      }
      const requestId = createMessageId("resolve-paths");
      return new Promise<PathResolution[]>((resolve) => {
        pendingPathResolutionsRef.current.set(requestId, { resolve });
        postIntent(vscodeApi, "resolvePaths", { paths: uniquePaths, requestId });
      });
    },
    [vscodeApi],
  );
  const handleSetBuildModel = useCallback(
    (modelId: string) => {
      postIntent(vscodeApi, "setBuildModel", {
        modelId,
      });
    },
    [vscodeApi],
  );
  const handleSetContextWindow = useCallback(
    (modelId: string, contextWindow: number) => {
      const sessionId = stateRef.current.activeSessionId;
      if (!sessionId || !modelId) {
        return;
      }
      postIntent(vscodeApi, "setContextWindow", { contextWindow, modelId, sessionId });
    },
    [vscodeApi],
  );
  const handleSetThinkingLevel = useCallback(
    (modelId: string, level: string) => {
      const sessionId = stateRef.current.activeSessionId;
      if (!sessionId || !modelId) {
        return;
      }
      postIntent(vscodeApi, "setThinkingLevel", { level, modelId, sessionId });
    },
    [vscodeApi],
  );

  const handleOpenDiff = useCallback(
    (toolCallId: string) => {
      postIntent(vscodeApi, "openDiff", {
        toolCallId,
      });
    },
    [vscodeApi],
  );
  const handleOpenPlanFile = useCallback(
    (path: string) => {
      postIntent(vscodeApi, "openPlanFile", {
        path,
      });
    },
    [vscodeApi],
  );
  const handleRetryUserMessage = useCallback(
    (messageId: string) => {
      if (!activeSession?.sessionId || !canPrompt) {
        return;
      }
      postIntent(vscodeApi, "retryUserMessage", {
        messageId,
        sessionId: activeSession.sessionId,
      });
    },
    [activeSession?.sessionId, canPrompt, vscodeApi],
  );
  const handleRecoverErrorTurn = useCallback(
    (errorId: string, action: "resume" | "retry") => {
      if (!activeSession?.sessionId || !canPrompt) {
        return;
      }
      postIntent(vscodeApi, "recoverErrorTurn", {
        action,
        errorId,
        sessionId: activeSession.sessionId,
      });
    },
    [activeSession?.sessionId, canPrompt, vscodeApi],
  );
  const handleOpenImagePreview = useCallback(
    (imageId: string) => {
      if (!activeSession?.sessionId) {
        return;
      }
      postIntent(vscodeApi, "openImagePreview", {
        attachmentId: imageId,
        sessionId: activeSession.sessionId,
      });
    },
    [activeSession?.sessionId, vscodeApi],
  );
  const handleZoomImage = useCallback((image: ZoomedImage) => {
    setZoomedImage(image);
  }, []);

  const handleContextSearchOpen = () => {
    setContextSearch({
      loading: true,
      matches: [],
      open: true,
      query: "",
      truncated: false,
    });
  };

  const handleContextSearchQueryChange = (query: string) => {
    // Keep the raw @query as a filename search term.
    // Line-scoped references continue to use the existing Add-to-Chat selection flow.
    setContextSearch((current) => ({
      ...current,
      loading: true,
      open: true,
      query,
      truncated: false,
    }));
  };

  const handleContextSearchClose = () => {
    latestContextSearchRequestIdRef.current = null;
    setContextSearch(EMPTY_CONTEXT_SEARCH_STATE);
  };

  const handleModeChange = (value: "chat" | "plan") => {
    if (!activeSession) {
      return;
    }
    const current = currentModeValue(activeSession.agentMode);
    if (value === current) {
      return;
    }
    postIntent(vscodeApi, "setPlanMode", {
      action: value === "plan" ? "enter" : "exit",
      planId: activeSession.activePlan?.planId ?? null,
      sessionId: activeSession.sessionId,
    });
  };

  const handleBuildPlan = useCallback(
    (planId: string | null, _path: string) => {
      if (!activeSession) {
        return;
      }
      postIntent(vscodeApi, "setPlanMode", {
        action: "build",
        planId,
        sessionId: activeSession.sessionId,
      });
    },
    [activeSession, vscodeApi],
  );

  const handleOpenRestoreDialog = useCallback(
    (checkpointId: string) => {
      if (!activeSession?.sessionId) {
        return;
      }
      const nextState = buildRestoreDialogState(
        activeSession.timeline,
        activeSession.checkpoints ?? [],
        activeSession.sessionId,
        checkpointId,
      );
      if (!nextState) {
        return;
      }
      setPendingRestoreDialog(nextState);
    },
    [activeSession],
  );

  const handleCancelRestore = () => {
    setPendingRestoreDialog(null);
  };

  const handleConfirmRestore = (revertFiles: boolean) => {
    if (!pendingRestoreDialog) {
      return;
    }
    if (pendingRestoreDialog.draft && pendingRestoreDialog.originalMessageId) {
      pendingRestoreRefillRef.current = {
        draft: pendingRestoreDialog.draft,
        originalMessageId: pendingRestoreDialog.originalMessageId,
        sessionId: pendingRestoreDialog.sessionId,
      };
    } else {
      pendingRestoreRefillRef.current = null;
    }
    postIntent(vscodeApi, "restoreCheckpoint", {
      checkpointId: pendingRestoreDialog.checkpointId,
      revertFiles,
      sessionId: pendingRestoreDialog.sessionId,
    });
    setPendingRestoreDialog(null);
  };

  const requestOlderHistory = () => {
    if (
      !activeSession?.sessionId ||
      activeSession.historyLoading === true ||
      activeSession.hasMoreHistory !== true
    ) {
      return;
    }
    postIntent(vscodeApi, "loadOlderHistory", {
      sessionId: activeSession.sessionId,
    });
  };

  useEffect(() => {
    if (bootstrapFillRef.current.sessionId !== (activeSession?.sessionId ?? null)) {
      bootstrapFillRef.current = {
        requestCount: 0,
        sessionId: activeSession?.sessionId ?? null,
      };
    }
    if (topPaginationRef.current.sessionId !== (activeSession?.sessionId ?? null)) {
      topPaginationRef.current = {
        active: false,
        anchorOldestItemId: null,
        sessionId: activeSession?.sessionId ?? null,
      };
    }
  }, [activeSession?.sessionId]);

  useEffect(() => {
    const stream = streamRef.current;
    if (!stream || !activeSession?.sessionId) {
      return;
    }
    if (activeSession.historyLoading === true) {
      return;
    }
    if (activeSession.hasMoreHistory !== true) {
      topPaginationRef.current.active = false;
      topPaginationRef.current.anchorOldestItemId = null;
      return;
    }
    const renderableNonEmpty = activeTimeline.length > 0 || activeApprovalCount > 0;
    if (!renderableNonEmpty) {
      requestOlderHistory();
      return;
    }
    if (topPaginationRef.current.active) {
      if (oldestTimelineItemId === topPaginationRef.current.anchorOldestItemId) {
        requestOlderHistory();
        return;
      }
      topPaginationRef.current.active = false;
      topPaginationRef.current.anchorOldestItemId = null;
    }
    if (stream.scrollHeight < stream.clientHeight * 0.9) {
      if (bootstrapFillRef.current.requestCount >= MAX_BOOTSTRAP_FILL_REQUESTS) {
        return;
      }
      bootstrapFillRef.current.requestCount += 1;
      requestOlderHistory();
      return;
    }
    bootstrapFillRef.current.requestCount = 0;
  }, [
    activeApprovalCount,
    activeSession?.hasMoreHistory,
    activeSession?.historyLoading,
    activeSession?.sessionId,
    activeTimeline.length,
  ]);

  useEffect(() => {
    const stream = streamRef.current;
    if (!stream) {
      return;
    }
    const handleScroll = () => {
      const nearTop = stream.scrollTop <= TOP_HISTORY_THRESHOLD_PX;
      topPaginationRef.current.active = nearTop;
      topPaginationRef.current.anchorOldestItemId = nearTop ? oldestTimelineItemId : null;
      if (nearTop) {
        requestOlderHistory();
      }
    };
    stream.addEventListener("scroll", handleScroll);
    return () => {
      stream.removeEventListener("scroll", handleScroll);
    };
  }, [
    activeSession?.hasMoreHistory,
    activeSession?.historyLoading,
    activeSession?.sessionId,
    oldestTimelineItemId,
  ]);

  const handleNewSession = () => {
    if (pendingDraftForkRef.current) return;
    const sourceSessionId = stateRef.current.activeSessionId;
    if (!sourceSessionId) {
      postIntent(vscodeApi, "newSession");
      return;
    }
    const draft = composerRef.current?.getDraft() ?? {
      hasContent: false,
      segments: [],
      text: "",
    };
    pendingDraftForkRef.current = {
      clickDraft: {
        ...draft,
        segments: draft.segments.map((segment) => ({ ...segment })),
      },
      cutoff: composerWorkRegistryRef.current.cutoff(sourceSessionId),
      operationId: null,
      sourceSessionId,
    };
    setDraftForkFeedback({ error: null, pending: true });
    postIntent(vscodeApi, "newSession");
  };

  return (
    <main className="tc-shell">
      <SessionBar
        activeSessionId={activeSession?.sessionId ?? null}
        canCompact={Boolean(activeSession && !activeSession.busy)}
        creating={draftForkFeedback.pending}
        onCompact={() => {
          if (!activeSession || activeSession.busy) return;
          postIntent(vscodeApi, "compact", { sessionId: activeSession.sessionId });
        }}
        onNewSession={handleNewSession}
        connectionStatus={state.connectionStatus}
        ready={state.ready}
        onSwitchSession={(sessionId) => {
          if (draftForkFeedback.pending) return;
          const activeSessionId = stateRef.current.activeSessionId;
          if (activeSessionId) {
            flushComposerDraft(activeSessionId);
          }
          postIntent(vscodeApi, "switchSession", {
            sessionId,
          });
        }}
        pendingQuestionCounts={pendingQuestionCounts}
        sessions={state.sessions}
      />
      <div
        aria-atomic="true"
        aria-live="polite"
        className="tc-visually-hidden"
        data-testid="question-announcement"
        role="status"
      >
        {questionAnnouncement}
      </div>
      {draftForkFeedback.error ? (
        <div
          className="tc-session-create-feedback"
          data-testid="draft-fork-error"
          role="alert"
        >
          {draftForkFeedback.error}
        </div>
      ) : null}

      <div className="tc-stream-shell">
        <section className="tc-stream" data-testid="stream-container" ref={streamRef}>
          <div className="tc-history-loader-slot">
            {activeSession?.historyLoading ? (
              <span className="tc-history-loader" data-testid="history-loader">
                Loading earlier…
              </span>
            ) : null}
          </div>
          {stickyUserMessageText ? (
            <StickyUserPrompt text={stickyUserMessageText} />
          ) : null}
          {!state.ready ? (
            <div className="tc-empty-state tc-empty-state--loading" data-testid="loading-state">
              <span className="tc-spinner" aria-hidden="true" />
              <p>
                {state.connectionStatus === "reconnecting"
                  ? "Reconnecting…"
                  : state.connectionStatus === "degraded"
                    ? "Connected, but initialization failed. Choose Retry in the notification."
                  : state.connectionStatus === "failed"
                    ? "Unable to connect. Choose Retry in the notification."
                    : "Connecting…"}
              </p>
            </div>
          ) : activeSession ? (
            activeSession.timeline.length ||
            activeApprovalCount ||
            activeSession.historyLoading ||
            activeSession.hasMoreHistory ? (
              <TranscriptView
                approvalAnswers={approvalAnswers}
                availableModelDetails={state.availableModelDetails}
                availableModels={state.availableModels}
                buildModel={state.buildModel ?? ""}
                busy={!!activeSession.busy}
                bottomSpacerHeight={bottomSpacerHeight}
                mediaRoots={state.mediaRoots}
                onAnswer={handleAnswerQuestion}
                onApprovalDraftChange={handleApprovalDraftChange}
                onSetBuildModel={handleSetBuildModel}
                onSelectContextWindow={handleSetContextWindow}
                onSelectThinkingLevel={handleSetThinkingLevel}
                checkpoints={activeSession.checkpoints ?? []}
                onOpenDiff={handleOpenDiff}
                onOpenFile={handleOpenFile}
                onOpenLink={handleOpenLink}
                onOpenImagePreview={handleOpenImagePreview}
                onOpenPlanFile={handleOpenPlanFile}
                resolvePaths={resolvePaths}
                onRecoverErrorTurn={handleRecoverErrorTurn}
                onRetryUserMessage={handleRetryUserMessage}
                canBuildPlan={canBuildPlan}
                onBuildPlan={handleBuildPlan}
                planId={activeSession.activePlan?.planId}
                planState={activeSession.activePlan?.state}
                planTodos={activeSession.planTodos ?? []}
                onRestoreCheckpoint={handleOpenRestoreDialog}
                sessionContextWindow={activeSessionContextWindow}
                sessionModel={activeSession.model ?? ""}
                sessionThinkingLevel={activeSession.thinkingLevel}
                sessionTodos={activeSession.sessionTodos ?? []}
                timeline={activeSession.timeline.filter(
                  (item) => item.type !== "approval" || !item.live,
                )}
                transcriptRef={transcriptRef}
                onZoomImage={handleZoomImage}
              />
            ) : (
              <div className="tc-empty-state">
                <h2>Ready to chat</h2>
                <p>Use the composer below to talk with Tomcat, switch models, or enter plan mode.</p>
              </div>
            )
          ) : (
            <div className="tc-empty-state">
              <h2>Ready to chat</h2>
              <p>Use the composer below to talk with Tomcat, switch models, or enter plan mode.</p>
            </div>
          )}
        </section>
        {userHasScrolled ? (
          <button
            aria-label="Jump to latest"
            className="tc-scroll-jump"
            data-testid="scroll-to-bottom"
            onClick={scrollToLatest}
            type="button"
          >
            <span aria-hidden="true" className="codicon codicon-arrow-down" />
          </button>
        ) : null}
      </div>

      {activeSession
        ? activeSession.timeline
            .filter(
              (item): item is WebviewApprovalCard =>
                item.type === "approval" && !item.resolved && item.live,
            )
            .map((item) => {
              const answerState = approvalAnswers[
                approvalAnswerKey(item.sessionId ?? activeSession.sessionId, item.request.requestId)
              ];
              return (
                <section
                  aria-label="Needs your answer"
                  className="tc-pending-question-panel"
                  data-pending-session-id={item.sessionId ?? activeSession.sessionId}
                  data-testid="pending-question-panel"
                  key={item.id}
                >
                  <h2 className="tc-visually-hidden">Needs your answer</h2>
                  <ApprovalCard
                    draft={answerState?.draft}
                    item={item}
                    onAnswer={handleAnswerQuestion}
                    onDraftChange={handleApprovalDraftChange}
                    submitting={answerState?.submitting}
                  />
                </section>
              );
            })
        : null}

      <TodoListWidget
        busy={!!activeSession?.busy}
        planState={activeSession?.activePlan?.state}
        planTodos={activeSession?.planTodos ?? []}
        sessionTodos={activeSession?.sessionTodos ?? []}
      />

      <AttachmentStrip
        attachments={activeSession?.pendingAttachments ?? []}
        onOpen={(attachment) => {
          if (attachment.kind === "image") {
            postIntent(vscodeApi, "openImagePreview", {
              attachmentId: attachment.id,
              sessionId: activeSession?.sessionId ?? "",
            });
            return;
          }
          if (attachment.path) {
            handleOpenFile(attachment.path);
          }
        }}
        onRemove={(attachmentId) =>
          postIntent(vscodeApi, "removeDraftAttachment", {
            attachmentId,
            sessionId: activeSession?.sessionId ?? "",
          })
        }
      />
      {imageAttachmentFeedback ? (
        <div
          className={
            imageAttachmentFeedback.hasErrors
              ? "tc-attachment-feedback tc-attachment-feedback--error"
              : // Success is announced but not shown: the images are already visible in
                // the strip above, so a banner saying so is redundant on screen while
                // still being the only signal a screen reader user gets.
                "tc-visually-hidden"
          }
          data-testid={
            imageAttachmentFeedback.hasErrors
              ? "attachment-feedback-error"
              : "attachment-feedback-announcement"
          }
          role="status"
        >
          {imageAttachmentFeedback.message}
        </div>
      ) : null}

      {pendingRestoreDialog ? (
        <RestoreConfirmDialog
          changedFiles={pendingRestoreDialog.changedFiles}
          onCancel={handleCancelRestore}
          onDontRevert={() => handleConfirmRestore(false)}
          onRevert={() => handleConfirmRestore(true)}
        />
      ) : null}
      <ImageLightbox image={zoomedImage} onClose={() => setZoomedImage(null)} />

      <Composer
        availableModelDetails={state.availableModelDetails}
        availableModelReasoningLevels={state.availableModelReasoningLevels}
        availableModels={state.availableModels}
        busy={!!activeSession?.busy}
        canInterrupt={canInterrupt}
        canPrompt={canPrompt}
        contextSearchLoading={contextSearch.loading}
        contextSearchMatches={contextSearch.matches}
        contextSearchQuery={contextSearch.query}
        contextSearchTruncated={contextSearch.truncated}
        contextWindowValue={activeSessionContextWindow}
        contextLabel={buildContextLabel(activeSession?.contextRatio)}
        modelCapabilities={activeModelCapabilities}
        modeValue={currentModeValue(activeSession?.agentMode)}
        modelValue={activeSession?.model ?? ""}
        thinkingLevelValue={activeSession?.thinkingLevel ?? ""}
        ref={composerRef}
        onContextSearchClose={handleContextSearchClose}
        onContextSearchOpen={handleContextSearchOpen}
        onContextSearchQueryChange={handleContextSearchQueryChange}
        onPickContext={() => {
          const sessionId = stateRef.current.activeSessionId;
          if (!sessionId) return;
          const ticket = composerWorkRegistryRef.current.begin(sessionId, "picker");
          postIntent(vscodeApi, "pickContext", {
            operationId: ticket.operationId,
            sessionId,
          });
        }}
        onContextWindowChange={handleSetContextWindow}
        onDraftChange={(draft) => {
          const sessionId = stateRef.current.activeSessionId;
          if (applyingBackendDraftRef.current || !sessionId) {
            return;
          }
          localComposerDraftsRef.current.set(sessionId, draft);
          localDraftRevisionsRef.current.set(
            sessionId,
            (localDraftRevisionsRef.current.get(sessionId) ?? 0) + 1,
          );
          scheduleComposerDraftSync(sessionId, draft);
        }}
        onModeChange={handleModeChange}
        onModelChange={(modelId) => {
          if (!activeSession || !modelId) {
            return;
          }
          postIntent(vscodeApi, "setModel", {
            modelId,
            sessionId: activeSession.sessionId,
          });
        }}
        onOpenModelSettings={modelAdminSupported
          ? () => {
              postIntent(vscodeApi, "openModelSettings", {
                route: "models",
              });
            }
          : undefined}
        onThinkingLevelChange={handleSetThinkingLevel}
        onPrepareAttachments={(work) => {
          const sessionId = stateRef.current.activeSessionId;
          if (!sessionId) return;
          const ticket = composerWorkRegistryRef.current.begin(sessionId, "paste");
          void work.then((files) => {
            postIntent(vscodeApi, "attachFiles", {
              files,
              operationId: ticket.operationId,
              sessionId,
            });
          }).catch(() => {
            // Composer restores text fallback; no host work was started.
            composerWorkRegistryRef.current.complete(ticket.operationId);
          });
        }}
        onResolveDrop={(uris) => {
          const sessionId = stateRef.current.activeSessionId;
          if (!sessionId) return;
          const ticket = composerWorkRegistryRef.current.begin(sessionId, "drop");
          postIntent(vscodeApi, "resolveDrop", {
            operationId: ticket.operationId,
            sessionId,
            uris,
          });
        }}
        onInterrupt={() => {
          if (!activeSession?.sessionId) {
            return;
          }
          postIntent(vscodeApi, "interrupt", {
            sessionId: activeSession.sessionId,
          });
        }}
        onSubmit={() => {
          flushComposerDraft();
          submitPrompt(
            vscodeApi,
            composerRef.current,
            activeSession?.sessionId,
            canPrompt,
            (pending) => {
              pendingComposerSubmissionRef.current = {
                ...pending,
                revision: pending.sessionId
                  ? (localDraftRevisionsRef.current.get(pending.sessionId) ?? 0)
                  : 0,
              };
            },
          );
        }}
        planState={activeSession?.activePlan?.state}
      />
    </main>
  );
}
