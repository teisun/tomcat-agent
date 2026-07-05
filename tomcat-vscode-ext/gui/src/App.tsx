import { useEffect, useMemo, useRef, useState } from "react";

import { AttachmentChips } from "./components/AttachmentChips";
import { Composer, type ComposerDraft, type ComposerHandle } from "./components/Composer";
import { SessionBar } from "./components/SessionBar";
import { StickyUserPrompt } from "./components/StickyUserPrompt";
import { TodoListWidget } from "./components/TodoListWidget";
import { TranscriptView } from "./components/TranscriptView";
import { isWebviewReference } from "./contextReferences";
import type {
  AskQuestionResult,
  HostToWebviewFrame,
  VsCodeApiLike,
  WebviewDomAction,
  WebviewMessageBlock,
  WebviewIntent,
  WebviewReference,
  WebviewStateSnapshot,
} from "./types";
import { useAutoScroll } from "./useAutoScroll";

const EMPTY_STATE: WebviewStateSnapshot = {
  activeSessionId: null,
  availableModelCapabilities: {},
  availableModels: [],
  ready: false,
  sessionViews: {},
  sessions: [],
  uiMode: "both",
};

const MAX_BOOTSTRAP_FILL_REQUESTS = 4;
const TOP_HISTORY_THRESHOLD_PX = 24;
const EMPTY_DRAFT: ComposerDraft = {
  hasContent: false,
  segments: [],
  text: "",
};

interface PendingComposerSubmission {
  draft: ComposerDraft;
  messageId: string;
  sessionId: string | null;
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
  const fileChipTopWithinStream =
    streamRect && fileChipRect ? fileChipRect.top - streamRect.top : null;
  const fileChipVisible =
    !!streamRect &&
    !!fileChipRect &&
    fileChipRect.bottom > streamRect.top &&
    fileChipRect.top < streamRect.bottom;
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
    hasConflict: !!document.querySelector('[data-testid="conflict-banner"]'),
    historyLoaderVisible: !!document.querySelector('[data-testid="history-loader"]'),
    html: root?.innerHTML ?? "",
    jumpToLatestVisible: !!document.querySelector('[data-testid="scroll-to-bottom"]'),
    latestUserTopWithinStream:
      streamRect && latestUserRect ? latestUserRect.top - streamRect.top : null,
    messageTexts: queryText('[data-testid="message-text"]'),
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
  };
}

function runDomAction(action: WebviewDomAction): void {
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
    const target = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(
      `[data-testid="${action.testId ?? ""}"]`,
    );
    if (!target) {
      return;
    }
    const nextValue = action.value ?? "";
    const descriptor = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(target),
      "value",
    );
    descriptor?.set?.call(target, nextValue);
    target.dispatchEvent(new Event("input", { bubbles: true }));
    target.dispatchEvent(new Event("change", { bubbles: true }));
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
    target.focus();
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
  const stateRef = useRef<WebviewStateSnapshot>(EMPTY_STATE);
  const composerRef = useRef<ComposerHandle | null>(null);
  const pendingInsertionsRef = useRef<Array<{ reference: WebviewReference; sessionId: string }>>([]);
  const pendingComposerSubmissionRef = useRef<PendingComposerSubmission | null>(null);
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
  const latestUserMessage = [...activeTimeline]
    .reverse()
    .find((item): item is WebviewMessageBlock => item.type === "message" && item.kind === "user") ?? null;
  const latestUserMessageText = latestUserMessage?.text ?? null;
  const lastTimelineItem = activeTimeline.at(-1);
  const lastItemIsLatestUser =
    lastTimelineItem?.type === "message" && lastTimelineItem.kind === "user";
  const userMessageCount = activeTimeline.filter(
    (item) => item.type === "message" && item.kind === "user",
  ).length;
  const streamContentKey = `${activeSession?.sessionId ?? "none"}:${activeTimeline.length}:${activeApprovalCount}`;
  const readOnlyConflict = activeSession?.conflictMessage ?? null;
  const canPrompt = state.uiMode !== "participant" && !activeSession?.busy && !readOnlyConflict;
  const canBuildPlan = !!activeSession && !activeSession.busy && !readOnlyConflict;
  const activeModelCapabilities = activeSession?.model
    ? state.availableModelCapabilities?.[activeSession.model]
    : undefined;
  const { bottomSpacerHeight, latestUserScrolledPast, scrollToLatest, userHasScrolled } = useAutoScroll({
    containerRef: streamRef,
    contentRef: transcriptRef,
    contentKey: streamContentKey,
    lastItemIsLatestUser,
    oldestItemKey: oldestTimelineItemId,
    resetKey: activeSession?.sessionId ?? null,
    userMessageCount,
  });

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

      {state.uiMode === "participant" ? (
        <section className="tc-banner tc-banner--warning" data-testid="disabled-banner">
          The Tomcat webview is disabled by `tomcat.ui=participant`.
        </section>
      ) : null}
      {readOnlyConflict ? (
        <section className="tc-banner tc-banner--warning" data-testid="conflict-banner">
          {readOnlyConflict}
        </section>
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
          {latestUserScrolledPast && latestUserMessageText ? (
            <StickyUserPrompt text={latestUserMessageText} />
          ) : null}
          {activeSession ? (
            activeSession.timeline.length ||
            activeApprovalCount ||
            activeSession.historyLoading ||
            activeSession.hasMoreHistory ? (
              <TranscriptView
                busy={!!activeSession.busy}
                bottomSpacerHeight={bottomSpacerHeight}
                onAnswer={handleAnswerQuestion}
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
            className="tc-scroll-jump"
            data-testid="scroll-to-bottom"
            onClick={scrollToLatest}
            type="button"
          >
            Jump to latest
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

      <Composer
        availableModels={state.availableModels}
        busy={!!activeSession?.busy}
        canPrompt={canPrompt}
        contextLabel={buildContextLabel(activeSession?.contextRatio)}
        modelCapabilities={activeModelCapabilities}
        modeValue={currentModeValue(activeSession?.planState)}
        modelValue={activeSession?.model ?? ""}
        thinkingLevelValue={normalizeThinkingLevel(activeSession?.thinkingLevel)}
        ref={composerRef}
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
