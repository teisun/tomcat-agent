import { useEffect, useMemo, useRef, useState } from "react";

import { AttachmentChips } from "./components/AttachmentChips";
import { injectCheckpointMarkers } from "./components/checkpointMarkers";
import { Composer, type ComposerDraft, type ComposerHandle } from "./components/Composer";
import { RestoreConfirmDialog } from "./components/RestoreConfirmDialog";
import { SessionBar } from "./components/SessionBar";
import { StickyUserPrompt } from "./components/StickyUserPrompt";
import { TodoListWidget } from "./components/TodoListWidget";
import { TranscriptView } from "./components/TranscriptView";
import { readContextSearchDebounceMs } from "./contextSearchConfig";
import { isWebviewReference } from "./contextReferences";
import type {
  AskQuestionResult,
  ContextSearchMatch,
  HostToWebviewFrame,
  VsCodeApiLike,
  WebviewDomAction,
  WebviewMessageBlock,
  WebviewIntent,
  WebviewReference,
  WebviewCheckpoint,
  WebviewTimelineItem,
  WebviewStateSnapshot,
} from "./types";
import { useAutoScroll } from "./useAutoScroll";

const EMPTY_STATE: WebviewStateSnapshot = {
  activeSessionId: null,
  availableModelCapabilities: {},
  availableModels: [],
  modelAdminSupported: false,
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
  const ctxLabel =
    document.querySelector<HTMLElement>('[data-testid="context-ratio"]')?.textContent ?? null;
  const planCardTodoCountText =
    document.querySelector<HTMLElement>('[data-testid="plan-todos-count"]')?.textContent ?? null;
  const planCardTitleText =
    document.querySelector<HTMLElement>('[data-testid="plan-card-title"]')?.textContent ?? null;
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
    html: root?.innerHTML ?? "",
    jumpToLatestVisible: !!document.querySelector('[data-testid="scroll-to-bottom"]'),
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
    toolBodyMetrics,
    toolTitles: queryText('[data-testid="tool-row-label"]'),
    assistantResponseGroups: transcriptGroups.length,
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
    planTodos: document.querySelectorAll('[data-testid^="plan-todo-"]').length,
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
  };
}

function runDomAction(action: WebviewDomAction): void {
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
  const resolveActionTarget = (): HTMLElement | null => {
    const nodes = [...document.querySelectorAll<HTMLElement>(`[data-testid="${action.testId ?? ""}"]`)];
    const resolvedIndex =
      typeof action.index === "number" && action.index < 0
        ? nodes.length + action.index
        : (action.index ?? 0);
    return nodes[resolvedIndex] ?? null;
  };

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
    if (target?.isContentEditable) {
      target.focus();
      const pasteEvent = new Event("paste", {
        bubbles: true,
        cancelable: true,
      }) as ClipboardEvent;
      Object.defineProperty(pasteEvent, "clipboardData", {
        configurable: true,
        value: {
          getData(format: string) {
            return format === "text/plain" ? nextValue : "";
          },
        },
      });
      target.dispatchEvent(pasteEvent);
      return;
    }
    dispatchTestComposerValue(nextValue);
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
  requestId: string,
  result: AskQuestionResult,
): void {
  postIntent(vscodeApi, "answerQuestion", {
    requestId,
    result,
  });
}

function buildContextLabel(contextRatio?: number | null): string {
  if (typeof contextRatio !== "number" || Number.isNaN(contextRatio)) {
    return "Ctx —";
  }
  return `Ctx ${Math.round(contextRatio * 100)}%`;
}

function normalizeThinkingLevel(
  thinkingLevel?: string | null,
): "" | "high" | "low" | "medium" | "xhigh" {
  switch (thinkingLevel) {
    case "low":
    case "medium":
    case "high":
    case "xhigh":
      return thinkingLevel;
    default:
      return "";
  }
}

function currentModeValue(planState?: string | null): "chat" | "plan" {
  return planState && planState !== "chat" ? "plan" : "chat";
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
  const [contextSearch, setContextSearch] = useState<ContextSearchState>(
    EMPTY_CONTEXT_SEARCH_STATE,
  );
  const [pendingRestoreDialog, setPendingRestoreDialog] = useState<PendingRestoreDialogState | null>(
    null,
  );
  const stateRef = useRef<WebviewStateSnapshot>(EMPTY_STATE);
  const composerRef = useRef<ComposerHandle | null>(null);
  const pendingInsertionsRef = useRef<Array<{ reference: WebviewReference; sessionId: string }>>([]);
  const pendingComposerSubmissionRef = useRef<PendingComposerSubmission | null>(null);
  const pendingRestoreRefillRef = useRef<PendingRestoreRefill | null>(null);
  const contextSearchRequestSeqRef = useRef(0);
  const latestContextSearchRequestIdRef = useRef<string | null>(null);
  const contextSearchWarningShownRef = useRef(false);
  const streamRef = useRef<HTMLElement | null>(null);
  const transcriptRef = useRef<HTMLElement | null>(null);

  const activeSession = useMemo(
    () =>
      state.activeSessionId
        ? state.sessionViews[state.activeSessionId]
        : undefined,
    [state.activeSessionId, state.sessionViews],
  );
  stateRef.current = state;

  const activeApprovalCount =
    activeSession?.timeline.filter((item) => item.type === "approval" && !item.resolved).length ?? 0;
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
  const canPrompt = !activeSession?.busy;
  const canInterrupt = true;
  const canBuildPlan = !!activeSession && !activeSession.busy;
  const modelAdminSupported = state.modelAdminSupported;
  const activeModelCapabilities = activeSession?.model
    ? state.availableModelCapabilities?.[activeSession.model]
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
    if (
      pendingRestoreRefillRef.current &&
      pendingRestoreRefillRef.current.sessionId !== (activeSession?.sessionId ?? "")
    ) {
      pendingRestoreRefillRef.current = null;
    }
  }, [activeSession?.sessionId]);

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
    const composer = composerRef.current;
    if (!pending || !composer) {
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
    if (state.activeSessionId !== resolved.sessionId) {
      return;
    }
    if (draftsEqual(composer.getDraft(), pending.draft)) {
      composer.clear();
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
    const handleMessage = (event: MessageEvent<HostToWebviewFrame>) => {
      const frame = event.data;
      if (!frame || typeof frame !== "object") {
        return;
      }
      if (frame.channel === "state") {
        stateRef.current = frame.content;
        setState(frame.content);
        vscodeApi.setState?.(frame.content);
        flushPendingInsertions();
        return;
      }
      if (
        frame.channel === "event" &&
        isInsertReferenceEvent(frame.content)
      ) {
        const insertion = {
          reference: frame.content.reference,
          sessionId: frame.content.sessionId,
        };
        if (composerRef.current && insertion.sessionId === stateRef.current.activeSessionId) {
          composerRef.current.insertReference(insertion.reference);
        } else {
          pendingInsertionsRef.current.push(insertion);
        }
        return;
      }
      if (frame.channel === "event") {
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

  const handleAnswerQuestion = (requestId: string, result: AskQuestionResult) => {
    answerQuestion(vscodeApi, requestId, result);
  };

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
    const current = currentModeValue(activeSession.planState);
    if (value === current) {
      return;
    }
    postIntent(vscodeApi, "setPlanMode", {
      action: value === "plan" ? "enter" : "exit",
      planId: activeSession.planId ?? null,
      sessionId: activeSession.sessionId,
    });
  };

  const handleBuildPlan = (planId: string | null, _path: string) => {
    if (!activeSession) {
      return;
    }
    postIntent(vscodeApi, "setPlanMode", {
      action: "build",
      planId,
      sessionId: activeSession.sessionId,
    });
  };

  const handleOpenRestoreDialog = (checkpointId: string) => {
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
  };

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

  return (
    <main className="tc-shell">
      <SessionBar
        activeSessionId={activeSession?.sessionId ?? null}
        onNewSession={() => postIntent(vscodeApi, "newSession")}
        ready={state.ready}
        onSwitchSession={(sessionId) =>
          postIntent(vscodeApi, "switchSession", {
            sessionId,
          })
        }
        sessions={state.sessions}
      />

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
          {activeSession ? (
            activeSession.timeline.length ||
            activeApprovalCount ||
            activeSession.historyLoading ||
            activeSession.hasMoreHistory ? (
              <TranscriptView
                availableModels={state.availableModels}
                buildModel={state.buildModel ?? ""}
                busy={!!activeSession.busy}
                bottomSpacerHeight={bottomSpacerHeight}
                onAnswer={handleAnswerQuestion}
                onSetBuildModel={(modelId) =>
                  postIntent(vscodeApi, "setBuildModel", {
                    modelId,
                  })
                }
                checkpoints={activeSession.checkpoints ?? []}
                onOpenDiff={(toolCallId) =>
                  postIntent(vscodeApi, "openDiff", {
                    toolCallId,
                  })
                }
                onOpenFile={(path) =>
                  postIntent(vscodeApi, "openFile", {
                    path,
                  })
                }
                onOpenPlanFile={(path) =>
                  postIntent(vscodeApi, "openPlanFile", {
                    path,
                  })
                }
                onRetryUserMessage={(messageId) => {
                  if (!activeSession?.sessionId || !canPrompt) {
                    return;
                  }
                  postIntent(vscodeApi, "retryUserMessage", {
                    messageId,
                    sessionId: activeSession.sessionId,
                  });
                }}
                canBuildPlan={canBuildPlan}
                onBuildPlan={handleBuildPlan}
                planState={activeSession.planState}
                planTodos={activeSession.planTodos ?? []}
                onRestoreCheckpoint={handleOpenRestoreDialog}
                sessionTodos={activeSession.sessionTodos ?? []}
                timeline={activeSession.timeline}
                transcriptRef={transcriptRef}
              />
            ) : (
              <div className="tc-empty-state">
                <h2>Ready to chat</h2>
                <p>Use the composer below to talk with Tomcat, switch models, or enter plan mode.</p>
              </div>
            )
          ) : state.ready ? (
            <div className="tc-empty-state">
              <h2>Ready to chat</h2>
              <p>Use the composer below to talk with Tomcat, switch models, or enter plan mode.</p>
            </div>
          ) : (
            <div className="tc-empty-state tc-empty-state--loading" data-testid="loading-state">
              <span className="tc-spinner" aria-hidden="true" />
              <p>Connecting…</p>
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

      <TodoListWidget
        busy={!!activeSession?.busy}
        planState={activeSession?.planState}
        planTodos={activeSession?.planTodos ?? []}
        sessionTodos={activeSession?.sessionTodos ?? []}
      />

      <AttachmentChips
        attachments={activeSession?.pendingAttachments ?? []}
        onRemove={(attachmentId) =>
          postIntent(vscodeApi, "removeAttachment", {
            attachmentId,
            sessionId: activeSession?.sessionId ?? null,
          })
        }
      />

      {pendingRestoreDialog ? (
        <RestoreConfirmDialog
          changedFiles={pendingRestoreDialog.changedFiles}
          onCancel={handleCancelRestore}
          onDontRevert={() => handleConfirmRestore(false)}
          onRevert={() => handleConfirmRestore(true)}
        />
      ) : null}

      <Composer
        availableModels={state.availableModels}
        busy={!!activeSession?.busy}
        canInterrupt={canInterrupt}
        canPrompt={canPrompt}
        contextSearchLoading={contextSearch.loading}
        contextSearchMatches={contextSearch.matches}
        contextSearchQuery={contextSearch.query}
        contextSearchTruncated={contextSearch.truncated}
        contextLabel={buildContextLabel(activeSession?.contextRatio)}
        modelCapabilities={activeModelCapabilities}
        modeValue={currentModeValue(activeSession?.planState)}
        modelValue={activeSession?.model ?? ""}
        thinkingLevelValue={normalizeThinkingLevel(activeSession?.thinkingLevel)}
        ref={composerRef}
        onContextSearchClose={handleContextSearchClose}
        onContextSearchOpen={handleContextSearchOpen}
        onContextSearchQueryChange={handleContextSearchQueryChange}
        onPickContext={() =>
          postIntent(vscodeApi, "pickContext", {
            sessionId: activeSession?.sessionId ?? null,
          })
        }
        onDraftChange={() => undefined}
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
        onThinkingLevelChange={(level) => {
          if (!activeSession || !activeSession.model || !level) {
            return;
          }
          postIntent(vscodeApi, "setThinkingLevel", {
            level,
            modelId: activeSession.model,
            sessionId: activeSession.sessionId,
          });
        }}
        onResolveDrop={(uris) =>
          postIntent(vscodeApi, "resolveDrop", {
            sessionId: activeSession?.sessionId ?? null,
            uris,
          })
        }
        onInterrupt={() => {
          if (!activeSession?.sessionId) {
            return;
          }
          postIntent(vscodeApi, "interrupt", {
            sessionId: activeSession.sessionId,
          });
        }}
        onSubmit={() =>
          submitPrompt(
            vscodeApi,
            composerRef.current,
            activeSession?.sessionId,
            canPrompt,
            (pending) => {
              pendingComposerSubmissionRef.current = pending;
            },
          )
        }
        planState={activeSession?.planState}
      />
    </main>
  );
}
