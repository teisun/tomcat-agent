import { useEffect, useMemo, useState } from "react";

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

function renderApprovalCards(
  vscodeApi: VsCodeApiLike,
  activeSession: WebviewSessionSnapshot,
) {
  return activeSession.approvals
    .filter((approval) => !approval.resolved)
    .map((approval) => (
      <section key={approval.request.requestId} data-testid="approval-card">
        <h3>Approval</h3>
        {approval.request.questions.map((question) => (
          <div key={question.id}>
            <p>{question.prompt}</p>
            <div>
              {question.options.map((option) => (
                <button
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
                </button>
              ))}
              <button
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

  const activeSession = useMemo(
    () =>
      state.activeSessionId
        ? state.sessionViews[state.activeSessionId]
        : undefined,
    [state.activeSessionId, state.sessionViews],
  );

  useEffect(() => {
    const handleMessage = (event: MessageEvent<HostToWebviewFrame>) => {
      const frame = event.data;
      if (!frame || typeof frame !== "object") {
        return;
      }
      if (frame.channel === "state") {
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
          data: buildDomSnapshot(state),
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
  }, [state, vscodeApi]);

  return (
    <main>
      <header>
        <h1>Tomcat</h1>
        <p>{state.ready ? "Connected" : "Connecting..."}</p>
        {state.uiMode === "participant" ? (
          <p data-testid="disabled-banner">
            The Tomcat webview is disabled by `tomcat.ui=participant`.
          </p>
        ) : null}
      </header>

      <section>
        <button onClick={() => postIntent(vscodeApi, "newSession")} type="button">
          New Session
        </button>
        <button onClick={() => postIntent(vscodeApi, "listSessions")} type="button">
          Refresh Sessions
        </button>
        {activeSession ? (
          <button
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

      <section aria-label="sessions">
        <h2>Sessions</h2>
        <div>
          {state.sessions.map((session) => (
            <button
              data-testid="session-tab"
              key={session.sessionId}
              onClick={() =>
                postIntent(vscodeApi, "switchSession", {
                  sessionId: session.sessionId,
                })
              }
              type="button"
            >
              {sessionLabel(session)}
              {session.owner ? ` (${session.owner})` : ""}
            </button>
          ))}
        </div>
      </section>

      <section aria-label="controls">
        <h2>Controls</h2>
        <label>
          Model
          <select
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
        <div>
          <button
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
      </section>

      <section aria-label="prompt">
        <h2>Prompt</h2>
        <textarea
          onChange={(event) => setPrompt(event.target.value)}
          rows={4}
          value={prompt}
        />
        <div>
          <button
            onClick={() => {
              if (!prompt.trim()) {
                return;
              }
              postIntent(vscodeApi, "prompt", {
                sessionId: activeSession?.sessionId ?? null,
                text: prompt,
              });
              setPrompt("");
            }}
            type="button"
          >
            Send
          </button>
        </div>
      </section>

      {activeSession ? (
        <section aria-label="active-session">
          <h2>Active Session</h2>
          <p>Session: {activeSession.sessionId}</p>
          <p>Model: {activeSession.model ?? "n/a"}</p>
          <p>
            Plan: {activeSession.planState ?? "chat"}
            {activeSession.planId ? ` (${activeSession.planId})` : ""}
          </p>
          {activeSession.conflictMessage ? (
            <p data-testid="conflict-banner">{activeSession.conflictMessage}</p>
          ) : null}

          <div>
            {activeSession.messages.map((message) => (
              <article key={message.id}>
                <strong>{message.kind}</strong>
                <p data-testid="message-text">{message.text}</p>
              </article>
            ))}
          </div>

          <div>{renderApprovalCards(vscodeApi, activeSession)}</div>

          <div>
            {activeSession.tools.map((tool) => (
              <section key={tool.toolCallId}>
                <h3 data-testid="tool-title">
                  {tool.toolName} ({tool.status})
                </h3>
                {tool.summary ? <pre>{tool.summary}</pre> : null}
                {tool.display?.kind === "file" ? (
                  <div>
                    <button
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
              </section>
            ))}
          </div>
        </section>
      ) : (
        <section>
          <p>No active Tomcat session.</p>
        </section>
      )}
    </main>
  );
}
