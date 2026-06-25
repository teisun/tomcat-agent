import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { ActivePlanStrip } from "./components/ActivePlanStrip";
import { AttachmentChips } from "./components/AttachmentChips";
import { Composer } from "./components/Composer";
import { SessionBar } from "./components/SessionBar";
import { TranscriptView } from "./components/TranscriptView";
import type {
  AskQuestionResult,
  HostToWebviewFrame,
  VsCodeApiLike,
  WebviewDomAction,
  WebviewIntent,
  WebviewStateSnapshot,
} from "./types";
import { useAutoScroll } from "./useAutoScroll";

const EMPTY_STATE: WebviewStateSnapshot = {
  activeSessionId: null,
  availableModels: [],
  ready: false,
  sessionViews: {},
  sessions: [],
  uiMode: "both",
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
  const toolBodyMetrics = [...document.querySelectorAll<HTMLElement>('[data-testid="tool-card"]')].map(
    (card) => {
      const title = card.querySelector('[data-testid="tool-title"]')?.textContent ?? "";
      const body = card.querySelector<HTMLElement>('[data-testid="tool-body"]');
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
  const streamRect = stream?.getBoundingClientRect();
  const latestUserRect = latestUserMessage?.getBoundingClientRect();
  return {
    activeSessionId: state.activeSessionId,
    approvalCount: document.querySelectorAll('[data-testid="approval-card"]').length,
    approvalInputTestIds,
    approvalOptionStates,
    composerControlMetrics,
    composerRowCount,
    disabledTestIds,
    expandedThinkingCount: document.querySelectorAll('[data-testid="thinking-block"] pre').length,
    expandedToolTitles: toolBodyMetrics.filter((entry) => entry.expanded).map((entry) => entry.title),
    hasConflict: !!document.querySelector('[data-testid="conflict-banner"]'),
    html: root?.innerHTML ?? "",
    jumpToLatestVisible: !!document.querySelector('[data-testid="scroll-to-bottom"]'),
    latestUserTopWithinStream:
      streamRect && latestUserRect ? latestUserRect.top - streamRect.top : null,
    messageTexts: queryText('[data-testid="message-text"]'),
    overflowAnchor: stream?.style.overflowAnchor ?? null,
    sessionTabs: queryText('[data-testid="session-option"]'),
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
    toolTitles: queryText('[data-testid="tool-title"]'),
  };
}

function runDomAction(action: WebviewDomAction): void {
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

  if (action.kind === "clickTestId") {
    const nodes = [
      ...document.querySelectorAll<HTMLElement>(`[data-testid="${action.testId ?? ""}"]`),
    ];
    const resolvedIndex =
      typeof action.index === "number" && action.index < 0
        ? nodes.length + action.index
        : (action.index ?? 0);
    const target = nodes[resolvedIndex];
    if (!target) {
      return;
    }
    target.focus();
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
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
  prompt: string,
  activeSessionId: string | null | undefined,
  canPrompt: boolean,
  setPrompt: (next: string) => void,
): void {
  const text = prompt.trim();
  if (!canPrompt || !text) {
    return;
  }
  postIntent(vscodeApi, "prompt", {
    sessionId: activeSessionId ?? null,
    text,
  });
  setPrompt("");
}

export function App({ vscodeApi }: { vscodeApi: VsCodeApiLike }) {
  const [state, setState] = useState<WebviewStateSnapshot>(EMPTY_STATE);
  const [prompt, setPrompt] = useState("");
  const stateRef = useRef<WebviewStateSnapshot>(EMPTY_STATE);
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
  const userMessageCount = activeTimeline.filter(
    (item) => item.type === "message" && item.kind === "user",
  ).length;
  const streamContentKey = `${activeSession?.sessionId ?? "none"}:${activeTimeline.length}:${activeApprovalCount}`;
  const readOnlyConflict = activeSession?.conflictMessage ?? null;
  const canPrompt = state.uiMode !== "participant" && !activeSession?.busy && !readOnlyConflict;
  const promptPlaceholder =
    state.uiMode === "participant"
      ? "Set `tomcat.ui` to `both` or `webview` to chat here."
      : readOnlyConflict
        ? "This live session is currently read-only in the webview."
        : activeSession?.busy
          ? "Tomcat is responding..."
          : "Message Tomcat (Enter to send, Shift+Enter for newline)";
  const { bottomSpacerHeight, scrollToLatest, userHasScrolled } = useAutoScroll({
    containerRef: streamRef,
    contentRef: transcriptRef,
    contentKey: streamContentKey,
    resetKey: activeSession?.sessionId ?? null,
    userMessageCount,
  });

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

  const handlePromptKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    event.preventDefault();
    submitPrompt(
      vscodeApi,
      prompt,
      activeSession?.sessionId,
      canPrompt,
      setPrompt,
    );
  };

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

  const handleBuildPlan = () => {
    if (!activeSession) {
      return;
    }
    postIntent(vscodeApi, "setPlanMode", {
      action: "build",
      planId: activeSession.planId ?? null,
      sessionId: activeSession.sessionId,
    });
  };

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
          {activeSession ? (
            activeSession.timeline.length || activeApprovalCount ? (
              <TranscriptView
                busy={!!activeSession.busy}
                bottomSpacerHeight={bottomSpacerHeight}
                onAnswer={handleAnswerQuestion}
                onApplyEdit={(toolCallId) =>
                  postIntent(vscodeApi, "applyEdit", {
                    toolCallId,
                  })
                }
                onOpenDiff={(toolCallId) =>
                  postIntent(vscodeApi, "openDiff", {
                    toolCallId,
                  })
                }
                onOpenPlanFile={(path) =>
                  postIntent(vscodeApi, "openPlanFile", {
                    path,
                  })
                }
                timeline={activeSession.timeline}
                transcriptRef={transcriptRef}
              />
            ) : (
              <div className="tc-empty-state">
                <h2>Ready to chat</h2>
                <p>Use the composer below to talk with Tomcat, switch models, or enter plan mode.</p>
              </div>
            )
          ) : (
            <div className="tc-empty-state">
              <h2>No active Tomcat session</h2>
              <p>Create a new session to start chatting.</p>
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

      <ActivePlanStrip
        canBuild={
          !!activeSession &&
          !readOnlyConflict &&
          !!activeSession.planFile &&
          activeSession.planFile.state !== "executing" &&
          (activeSession.planFile.state === "planning" || activeSession.planFile.state === "pending")
        }
        onBuild={handleBuildPlan}
        onOpenPlanFile={(path) =>
          postIntent(vscodeApi, "openPlanFile", {
            path,
          })
        }
        planFile={activeSession?.planFile}
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
        canPrompt={canPrompt}
        contextLabel={buildContextLabel(activeSession?.contextRatio)}
        modeValue={currentModeValue(activeSession?.planState)}
        modelValue={activeSession?.model ?? ""}
        thinkingLevelValue={normalizeThinkingLevel(activeSession?.thinkingLevel)}
        onAddAttachment={() =>
          postIntent(vscodeApi, "pickAttachment", {
            sessionId: activeSession?.sessionId ?? null,
          })
        }
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
        onPromptChange={setPrompt}
        onPromptKeyDown={handlePromptKeyDown}
        onSubmit={() =>
          submitPrompt(
            vscodeApi,
            prompt,
            activeSession?.sessionId,
            canPrompt,
            setPrompt,
          )
        }
        planState={activeSession?.planState}
        prompt={prompt}
        promptPlaceholder={promptPlaceholder}
      />
    </main>
  );
}
