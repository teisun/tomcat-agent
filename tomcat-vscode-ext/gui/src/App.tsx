import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import type {
  HostToWebviewFrame,
  VsCodeApiLike,
  WebviewIntent,
  WebviewSessionSnapshot,
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

const MESSAGE_LABELS: Record<WebviewSessionSnapshot["messages"][number]["kind"], string> = {
  assistant: "Tomcat",
  error: "Error",
  notice: "Notice",
  thinking: "Thinking",
  user: "You",
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
    sessionTabs: queryText('[data-testid="session-tab"]'),
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

function sessionLabel(session: WebviewStateSnapshot["sessions"][number]): string {
  return session.isCurrent ? `${session.sessionId} *` : session.sessionId;
}

function compactSessionId(sessionId: string): string {
  if (sessionId.length <= 18) {
    return sessionId;
  }
  return `${sessionId.slice(0, 8)}...${sessionId.slice(-6)}`;
}

function formatPlanState(planState?: string | null): string {
  if (!planState) {
    return "Chat";
  }
  return planState
    .split("_")
    .map((segment) => segment.slice(0, 1).toUpperCase() + segment.slice(1))
    .join(" ");
}

function messageTone(kind: WebviewSessionSnapshot["messages"][number]["kind"]): string {
  return `tc-message tc-message--${kind}`;
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

function renderApprovalCards(
  vscodeApi: VsCodeApiLike,
  activeSession: WebviewSessionSnapshot,
) {
  return activeSession.approvals
    .filter((approval) => !approval.resolved)
    .map((approval) => (
      <section
        className="tc-card tc-approval-card"
        key={approval.request.requestId}
        data-testid="approval-card"
      >
        <div className="tc-card__header">
          <h3>Approval Required</h3>
          <span className="tc-chip tc-chip--warning">Pending</span>
        </div>
        {approval.request.questions.map((question) => (
          <div className="tc-approval-question" key={question.id}>
            <p>{question.prompt}</p>
            <div className="tc-button-row">
              {question.options.map((option) => (
                <button
                  className={
                    option.recommended ? "tc-button tc-button--primary" : "tc-button tc-button--secondary"
                  }
                  key={option.id}
                  onClick={() =>
                    answerQuestion(
                      vscodeApi,
                      approval.request.requestId,
                      question.id,
                      option.id,
                      !!option.recommended,
                    )
                  }
                  type="button"
                >
                  {option.label}
                  {option.recommended ? " (Recommended)" : ""}
                </button>
              ))}
              <button
                className="tc-button tc-button--ghost"
                onClick={() =>
                  answerQuestion(
                    vscodeApi,
                    approval.request.requestId,
                    question.id,
                    null,
                    false,
                  )
                }
                type="button"
              >
                Skip
              </button>
            </div>
          </div>
        ))}
      </section>
    ));
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

  const activeApprovalCount = activeSession?.approvals.filter((approval) => !approval.resolved).length ?? 0;
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

      <section className="tc-toolbar">
        <button
          className="tc-button tc-button--secondary"
          onClick={() => postIntent(vscodeApi, "newSession")}
          type="button"
        >
          New Session
        </button>
        <button
          className="tc-button tc-button--ghost"
          onClick={() => postIntent(vscodeApi, "listSessions")}
          type="button"
        >
          Refresh Sessions
        </button>
        {activeSession ? (
          <button
            className="tc-button tc-button--ghost"
            onClick={() =>
              postIntent(vscodeApi, "closeSession", {
                sessionId: activeSession.sessionId,
              })
            }
            type="button"
          >
            Close Active Session
          </button>
        ) : null}
      </section>

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

      <section className="tc-panel" aria-label="sessions">
        <div className="tc-panel__header">
          <h2>Sessions</h2>
          <span className="tc-chip">{state.sessions.length}</span>
        </div>
        <div className="tc-session-rail">
          {state.sessions.length ? (
            state.sessions.map((session) => (
              <button
                className={
                  session.sessionId === activeSession?.sessionId
                    ? "tc-session-tab tc-session-tab--active"
                    : "tc-session-tab"
                }
                data-testid="session-tab"
                key={session.sessionId}
                onClick={() =>
                  postIntent(vscodeApi, "switchSession", {
                    sessionId: session.sessionId,
                  })
                }
                title={session.sessionId}
                type="button"
              >
                <span>{sessionLabel(session)}</span>
                {session.owner ? <span className="tc-session-tab__meta"> ({session.owner})</span> : null}
                {session.busy ? <span className="tc-session-tab__meta"> running</span> : null}
              </button>
            ))
          ) : (
            <p className="tc-empty-inline">No sessions yet.</p>
          )}
        </div>
      </section>

      <section className="tc-panel" aria-label="controls">
        <div className="tc-panel__header">
          <h2>Controls</h2>
          {activeSession ? (
            <span className="tc-chip">{compactSessionId(activeSession.sessionId)}</span>
          ) : null}
        </div>
        <div className="tc-control-grid">
          <label className="tc-field">
            <span>Model</span>
            <select
              aria-label="Tomcat model"
              disabled={!activeSession || !!readOnlyConflict || !state.availableModels.length}
              onChange={(event) => {
                if (!activeSession || !event.target.value) {
                  return;
                }
                postIntent(vscodeApi, "setModel", {
                  modelId: event.target.value,
                  sessionId: activeSession.sessionId,
                });
              }}
              value={activeSession?.model ?? ""}
            >
              <option value="">Select model</option>
              {state.availableModels.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </label>
          <div className="tc-button-row">
            <button
              aria-label="Enter plan mode"
              className="tc-button tc-button--secondary"
              disabled={!activeSession || !!readOnlyConflict}
              onClick={() =>
                activeSession &&
                postIntent(vscodeApi, "setPlanMode", {
                  action: "enter",
                  sessionId: activeSession.sessionId,
                })
              }
              type="button"
            >
              Enter Plan
            </button>
            <button
              aria-label="Build plan"
              className="tc-button tc-button--secondary"
              disabled={!activeSession || !!readOnlyConflict}
              onClick={() =>
                activeSession &&
                postIntent(vscodeApi, "setPlanMode", {
                  action: "build",
                  sessionId: activeSession.sessionId,
                })
              }
              type="button"
            >
              Build Plan
            </button>
            <button
              aria-label="Exit plan mode"
              className="tc-button tc-button--ghost"
              disabled={!activeSession || !!readOnlyConflict || activeSession.planState === "chat"}
              onClick={() =>
                activeSession &&
                postIntent(vscodeApi, "setPlanMode", {
                  action: "exit",
                  sessionId: activeSession.sessionId,
                })
              }
              type="button"
            >
              Exit Plan
            </button>
            <button
              aria-label="Interrupt session"
              className="tc-button tc-button--ghost"
              disabled={!activeSession}
              onClick={() =>
                activeSession &&
                postIntent(vscodeApi, "interrupt", {
                  sessionId: activeSession.sessionId,
                })
              }
              type="button"
            >
              Interrupt
            </button>
          </div>
        </div>
        {activeSession ? (
          <div className="tc-chip-row">
            <span className="tc-chip">Model: {activeSession.model ?? "n/a"}</span>
            <span className="tc-chip">Plan: {formatPlanState(activeSession.planState)}</span>
            {activeSession.planId ? <span className="tc-chip">Plan ID: {activeSession.planId}</span> : null}
            {activeSession.busy ? <span className="tc-chip tc-chip--warning">Running</span> : null}
            {activeSession.owner ? <span className="tc-chip">Owner: {activeSession.owner}</span> : null}
          </div>
        ) : (
          <p className="tc-empty-inline">Start a session to chat with Tomcat.</p>
        )}
      </section>

      <section className="tc-stream" aria-label="active-session">
        {activeSession ? (
          <>
            {!activeSession.messages.length && !activeSession.tools.length && !activeApprovalCount ? (
              <div className="tc-empty-state">
                <h2>Ready to chat</h2>
                <p>
                  Use the composer below to talk with Tomcat, switch models, or enter plan mode.
                </p>
              </div>
            ) : null}

            {activeSession.messages.map((message) => (
              <article className={messageTone(message.kind)} key={message.id}>
                <div className="tc-message__header">
                  <strong>{MESSAGE_LABELS[message.kind]}</strong>
                  <span>{message.kind}</span>
                </div>
                <p data-testid="message-text">{message.text}</p>
              </article>
            ))}

            {activeSession.tools.map((tool) => (
              <section className="tc-card" key={tool.toolCallId}>
                <div className="tc-card__header">
                  <h3 data-testid="tool-title">
                    {tool.toolName} ({tool.status})
                  </h3>
                  <span
                    className={
                      tool.isError ? "tc-chip tc-chip--danger" : "tc-chip tc-chip--success"
                    }
                  >
                    {tool.isError ? "Error" : "Done"}
                  </span>
                </div>
                {tool.summary ? <pre>{tool.summary}</pre> : null}
                {tool.display?.kind === "file" ? (
                  <div className="tc-button-row">
                    <button
                      className="tc-button tc-button--secondary"
                      onClick={() =>
                        postIntent(vscodeApi, "openDiff", {
                          toolCallId: tool.toolCallId,
                        })
                      }
                      type="button"
                    >
                      Open Diff
                    </button>
                    <button
                      className="tc-button tc-button--primary"
                      onClick={() =>
                        postIntent(vscodeApi, "applyEdit", {
                          toolCallId: tool.toolCallId,
                        })
                      }
                      type="button"
                    >
                      Apply Edit
                    </button>
                  </div>
                ) : null}
                {tool.display?.kind === "plan" ? <pre>{tool.display.plan}</pre> : null}
                {tool.display?.kind === "text" ? <pre>{tool.display.text}</pre> : null}
              </section>
            ))}

            {renderApprovalCards(vscodeApi, activeSession)}
          </>
        ) : (
          <div className="tc-empty-state">
            <h2>No active Tomcat session</h2>
            <p>Create a new session or refresh the session list to start chatting.</p>
          </div>
        )}
      </section>

      <section className="tc-composer" aria-label="prompt">
        <label className="tc-field tc-field--composer">
          <span>Message</span>
          <textarea
            aria-label="Tomcat prompt"
            disabled={!canPrompt}
            onChange={(event) => setPrompt(event.target.value)}
            onKeyDown={handlePromptKeyDown}
            placeholder={promptPlaceholder}
            rows={3}
            value={prompt}
          />
        </label>
        <div className="tc-composer__footer">
          <span className="tc-composer__hint">{promptPlaceholder}</span>
          <button
            className="tc-button tc-button--primary"
            disabled={!prompt.trim() || !canPrompt}
            onClick={() =>
              submitPrompt(
                vscodeApi,
                prompt,
                activeSession?.sessionId,
                canPrompt,
                setPrompt,
              )
            }
            type="button"
          >
            Send
          </button>
        </div>
      </section>
    </main>
  );
}
