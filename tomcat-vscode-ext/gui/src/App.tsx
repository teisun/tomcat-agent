import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { ActivePlanStrip } from "./components/ActivePlanStrip";
import { AttachmentChips } from "./components/AttachmentChips";
import { Composer } from "./components/Composer";
import { SessionBar } from "./components/SessionBar";
import { TranscriptView } from "./components/TranscriptView";
import type {
  HostToWebviewFrame,
  VsCodeApiLike,
  WebviewIntent,
  WebviewStateSnapshot,
} from "./types";

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
  const queryText = (selector: string) =>
    [...document.querySelectorAll(selector)].map((node) => node.textContent ?? "");
  return {
    activeSessionId: state.activeSessionId,
    approvalCount: document.querySelectorAll('[data-testid="approval-card"]').length,
    hasConflict: !!document.querySelector('[data-testid="conflict-banner"]'),
    html: root?.innerHTML ?? "",
    messageTexts: queryText('[data-testid="message-text"]'),
    sessionTabs: queryText('[data-testid="session-option"]'),
    toolTitles: queryText('[data-testid="tool-title"]'),
  };
}

function answerQuestion(
  vscodeApi: VsCodeApiLike,
  requestId: string,
  questionId: string,
  optionId: string | null,
  pickedRecommended: boolean,
): void {
  postIntent(vscodeApi, "answerQuestion", {
    requestId,
    result: {
      answers: [
        {
          customText: null,
          optionIds: optionId ? [optionId] : [],
          pickedRecommended,
          questionId,
          skipped: optionId === null,
        },
      ],
      cancelled: false,
    },
  });
}

function buildContextLabel(contextRatio?: number | null): string {
  if (typeof contextRatio !== "number" || Number.isNaN(contextRatio)) {
    return "Ctx —";
  }
  return `Ctx ${Math.round(contextRatio * 100)}%`;
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

  const handleAnswerQuestion = (
    requestId: string,
    questionId: string,
    optionId: string | null,
    pickedRecommended: boolean,
  ) => {
    answerQuestion(vscodeApi, requestId, questionId, optionId, pickedRecommended);
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
      <header className="tc-header">
        <div>
          <p className="tc-header__eyebrow">Tomcat Chat</p>
          <h1>Tomcat</h1>
        </div>
        <span
          className={state.ready ? "tc-chip tc-chip--success" : "tc-chip tc-chip--warning"}
        >
          {state.ready ? "Connected" : "Connecting..."}
        </span>
      </header>

      <SessionBar
        activeSessionId={activeSession?.sessionId ?? null}
        onCloseSession={() =>
          activeSession &&
          postIntent(vscodeApi, "closeSession", {
            sessionId: activeSession.sessionId,
          })
        }
        onNewSession={() => postIntent(vscodeApi, "newSession")}
        onRefreshSessions={() => postIntent(vscodeApi, "listSessions")}
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

      <section className="tc-stream">
        {activeSession ? (
          activeSession.timeline.length || activeApprovalCount ? (
            <TranscriptView
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
            <p>Create a new session or refresh the session list to start chatting.</p>
          </div>
        )}
      </section>

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
