import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { HostToWebviewFrame, VsCodeApiLike } from "./types";

function mount() {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike = {
    postMessage,
    setState: vi.fn(),
  };
  render(<App vscodeApi={vscodeApi} />);
  return { postMessage, vscodeApi };
}

async function emitState(frame: HostToWebviewFrame) {
  await act(async () => {
    window.dispatchEvent(new MessageEvent("message", { data: frame }));
  });
}

async function emitReadySessionState(sessionId = "s1") {
  await emitState({
    channel: "state",
    content: {
      activeSessionId: sessionId,
      availableModels: ["gpt-5.4"],
      ready: true,
      sessions: [
        {
          busy: false,
          isCurrent: true,
          ownedByThisFrontend: true,
          owner: "webview",
          sessionId,
          title: null,
          updatedAt: 1,
        },
      ],
      sessionViews: {
        [sessionId]: {
          busy: false,
          conflictMessage: null,
          contextRatio: null,
          hasMoreHistory: false,
          historyLoading: false,
          model: "gpt-5.4",
          ownedByThisFrontend: true,
          owner: "webview",
          pendingAttachments: [],
          planFile: null,
          planId: null,
          planState: "chat",
          sessionId,
          thinkingLevel: "high",
          timeline: [],
        },
      },
      uiMode: "both",
    },
    messageId: `state-ready-${sessionId}`,
  });
}

beforeAll(() => {
  const emptyRect = () => ({
    bottom: 0,
    height: 0,
    left: 0,
    right: 0,
    toJSON() {
      return {};
    },
    top: 0,
    width: 0,
    x: 0,
    y: 0,
  });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: emptyRect,
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});

function mockScrollableTranscript({
  scrollHeight,
  scrollTop,
  userBottom,
  userTop,
}: {
  scrollHeight: number;
  scrollTop: number;
  userBottom: number;
  userTop: number;
}) {
  const stream = screen.getByTestId("stream-container");
  const transcript = screen.getByLabelText("active-session");
  const userMessage = screen.getAllByTestId("message-block").find(
    (node) => node.getAttribute("data-kind") === "user",
  );

  if (!userMessage) {
    throw new Error("Expected a user message in the transcript");
  }

  Object.defineProperty(stream, "clientHeight", {
    configurable: true,
    get: () => 100,
  });
  Object.defineProperty(stream, "scrollHeight", {
    configurable: true,
    get: () => scrollHeight,
  });
  Object.defineProperty(stream, "scrollTop", {
    configurable: true,
    get: () => scrollTop,
  });
  Object.defineProperty(transcript, "scrollHeight", {
    configurable: true,
    get: () => scrollHeight,
  });

  (stream as HTMLElement).getBoundingClientRect = vi.fn(
    () => ({ top: 0, bottom: 100, height: 100, left: 0, right: 0, width: 0, x: 0, y: 0 }) as DOMRect,
  );
  (transcript as HTMLElement).getBoundingClientRect = vi.fn(
    () =>
      ({
        top: -scrollTop,
        bottom: scrollHeight - scrollTop,
        height: scrollHeight,
        left: 0,
        right: 0,
        width: 0,
        x: 0,
        y: -scrollTop,
      }) as DOMRect,
  );
  (userMessage as HTMLElement).getBoundingClientRect = vi.fn(
    () =>
      ({
        top: userTop,
        bottom: userBottom,
        height: userBottom - userTop,
        left: 0,
        right: 0,
        width: 0,
        x: 0,
        y: userTop,
      }) as DOMRect,
  );
}

describe("Tomcat webview App", () => {
  it("shows a loading state while serve is connecting", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: null,
        availableModels: [],
        ready: false,
        sessions: [],
        sessionViews: {},
        uiMode: "both",
      },
      messageId: "state-loading",
    });

    expect(screen.getByTestId("loading-state").textContent).toContain("Connecting");
    expect(screen.queryByText("No active Tomcat session")).toBeNull();
    expect(screen.getByTestId("connection-chip").getAttribute("aria-label")).toContain(
      "Connecting",
    );
    expect(screen.getByTestId("connection-chip").className).toContain(
      "tc-conn-light--connecting",
    );
  });

  it("shows Ready to chat when connected but no active session", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: null,
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [],
        sessionViews: {},
        uiMode: "both",
      },
      messageId: "state-ready-empty",
    });

    expect(screen.getByText("Ready to chat")).toBeTruthy();
    expect(screen.queryByText("No active Tomcat session")).toBeNull();
    expect(screen.queryByTestId("loading-state")).toBeNull();
  });

  it("renders transcript timeline, plan UI, attachments, and context ratio", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4", "claude-4.6-sonnet"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: 0.42,
            hasMoreHistory: true,
            historyLoading: true,
            model: "gpt-5.4",
            thinkingLevel: "high",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [
              {
                attachment: {
                  dataBase64: "YWJj",
                  kind: "file",
                  mimeType: "text/markdown",
                },
                id: "att-1",
                kind: "file",
                label: "README.md",
                mimeType: "text/markdown",
                path: "/workspace/README.md",
              },
            ],
            planFile: {
              path: "/workspace/login-refactor.plan.md",
              planId: "plan-1",
              state: "planning",
            },
            planId: "plan-1",
            planState: "planning",
            sessionId: "s1",
            timeline: [
              {
                coveredCount: 12,
                id: "boundary-1",
                summary: "Earlier turns were compacted.",
                type: "boundary",
              },
              { id: "m2", text: "thinking...", type: "thinking" },
              { id: "m1", kind: "assistant", text: "hello", type: "message" },
              {
                display: { file: "src/app.ts", kind: "file" },
                id: "tool-card-1",
                isError: false,
                status: "complete",
                summary: "updated file",
                toolCallId: "tool-1",
                toolName: "edit",
                type: "tool",
              },
              {
                id: "plan-card-1",
                path: "/workspace/login-refactor.plan.md",
                planId: "plan-1",
                state: "planning",
                type: "plan",
              },
              {
                id: "approval-1",
                request: {
                  questions: [
                    {
                      id: "q1",
                      options: [
                        { id: "yes", label: "Yes", recommended: true },
                        { id: "no", label: "No" },
                      ],
                      prompt: "Proceed?",
                    },
                  ],
                  requestId: "r1",
                  responseEvent: "response",
                },
                resolved: false,
                sessionId: "s1",
                type: "approval",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-1",
    });

    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.getByTestId("history-loader").textContent).toContain("Loading earlier");
    expect(screen.getByTestId("boundary-block").textContent).toContain("Earlier history summary");
    expect(screen.getByTestId("thinking-summary").textContent).toContain("thinking...");
    expect(screen.queryByTestId("thinking-body")).toBeNull();
    fireEvent.click(screen.getByTestId("thinking-toggle"));
    expect(screen.getByTestId("thinking-body").textContent).toContain("thinking...");
    expect(screen.getByText("Questions")).toBeTruthy();
    expect(screen.getByText("Proceed?")).toBeTruthy();
    expect(screen.getByTestId("file-chip").textContent).toContain("app.ts");
    expect(screen.queryByText("updated file")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByText("updated file")).toBeTruthy();
    fireEvent.click(screen.getByTestId("session-select"));
    expect(screen.getByTestId("session-option").textContent).toContain("New session");
    expect(screen.queryByLabelText("Close active session")).toBeNull();
    expect(screen.getByTestId("plan-card").textContent).toContain("login-refactor.plan.md");
    expect(screen.getByTestId("build-plan").textContent).toContain("Build");
    expect(screen.getByTestId("attachment-chip").textContent).toContain("README.md");
    expect(screen.getByTestId("context-ratio").textContent).toContain("Ctx 42%");
  });

  it("requests older history when the transcript is underfilled or scrolled near the top", async () => {
    const { postMessage } = mount();
    const stream = screen.getByTestId("stream-container");
    let scrollTop = 0;

    Object.defineProperty(stream, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(stream, "scrollHeight", {
      configurable: true,
      get: () => 40,
    });
    Object.defineProperty(stream, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [
              { id: "m-user", kind: "user", text: "hello", type: "message" },
              { id: "m-assistant", kind: "assistant", text: "world", type: "message" },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-underfill",
    });

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { sessionId: "s1" },
        type: "loadOlderHistory",
      }),
    );

    postMessage.mockClear();
    scrollTop = 12;
    act(() => {
      fireEvent.scroll(stream);
    });
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { sessionId: "s1" },
        type: "loadOlderHistory",
      }),
    );

    postMessage.mockClear();
    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [
              { id: "m-user", kind: "user", text: "hello", type: "message" },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-no-more-history",
    });
    scrollTop = 8;
    act(() => {
      fireEvent.scroll(stream);
    });
    expect(postMessage).not.toHaveBeenCalled();
  });

  it("keeps requesting older history when the restored timeline is still empty", async () => {
    const { postMessage } = mount();
    const stream = screen.getByTestId("stream-container");

    Object.defineProperty(stream, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(stream, "scrollHeight", {
      configurable: true,
      get: () => 0,
    });
    Object.defineProperty(stream, "scrollTop", {
      configurable: true,
      get: () => 0,
      set: () => undefined,
    });

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-empty-history",
    });

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { sessionId: "s1" },
        type: "loadOlderHistory",
      }),
    );
  });

  it("keeps Build enabled for a restored planning card even when activeSession.planFile is null", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: "Restored plan session",
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "planning",
            sessionId: "s1",
            timeline: [
              {
                id: "plan-card-1",
                path: "/workspace/restored.plan.md",
                planId: "plan-restored",
                state: "planning",
                type: "plan",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-restored-plan-build",
    });

    expect((screen.getByTestId("build-plan") as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps top-pagination alive when older pages still do not advance the visible oldest item", async () => {
    const { postMessage } = mount();
    const stream = screen.getByTestId("stream-container");
    let scrollTop = 60;

    Object.defineProperty(stream, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(stream, "scrollHeight", {
      configurable: true,
      get: () => 220,
    });
    Object.defineProperty(stream, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [{ id: "visible-oldest", kind: "assistant", text: "chunk", type: "message" }],
          },
        },
        uiMode: "both",
      },
      messageId: "state-top-pagination-ready",
    });

    postMessage.mockClear();
    scrollTop = 0;
    act(() => {
      fireEvent.scroll(stream);
    });
    expect(postMessage).toHaveBeenCalledTimes(1);

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: true,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [{ id: "visible-oldest", kind: "assistant", text: "chunk", type: "message" }],
          },
        },
        uiMode: "both",
      },
      messageId: "state-top-pagination-loading",
    });

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [{ id: "visible-oldest", kind: "assistant", text: "chunk", type: "message" }],
          },
        },
        uiMode: "both",
      },
      messageId: "state-top-pagination-still-buffered",
    });

    const loadOlderCalls = postMessage.mock.calls.filter(
      ([message]) => message?.type === "loadOlderHistory",
    );
    expect(loadOlderCalls).toHaveLength(2);
  });

  it("stops bootstrap underfill requests at the safety cap", async () => {
    const { postMessage } = mount();
    const stream = screen.getByTestId("stream-container");
    let scrollTop = 0;

    Object.defineProperty(stream, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(stream, "scrollHeight", {
      configurable: true,
      get: () => 80,
    });
    Object.defineProperty(stream, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });

    for (let index = 0; index < 6; index += 1) {
      await emitState({
        channel: "state",
        content: {
          activeSessionId: "s1",
          availableModels: ["gpt-5.4"],
          ready: true,
          sessions: [
            {
              busy: false,
              isCurrent: true,
              ownedByThisFrontend: true,
              owner: "webview",
              sessionId: "s1",
              title: null,
              updatedAt: 1,
            },
          ],
          sessionViews: {
            s1: {
              busy: false,
              conflictMessage: null,
              hasMoreHistory: true,
              historyLoading: true,
              model: "gpt-5.4",
              ownedByThisFrontend: true,
              owner: "webview",
              pendingAttachments: [],
              planId: null,
              planState: "chat",
              sessionId: "s1",
              timeline: [{ id: `m-${index}`, kind: "assistant", text: "chunk", type: "message" }],
            },
          },
          uiMode: "both",
        },
        messageId: `state-underfill-loading-${index}`,
      });

      await emitState({
        channel: "state",
        content: {
          activeSessionId: "s1",
          availableModels: ["gpt-5.4"],
          ready: true,
          sessions: [
            {
              busy: false,
              isCurrent: true,
              ownedByThisFrontend: true,
              owner: "webview",
              sessionId: "s1",
              title: null,
              updatedAt: 1,
            },
          ],
          sessionViews: {
            s1: {
              busy: false,
              conflictMessage: null,
              hasMoreHistory: true,
              historyLoading: false,
              model: "gpt-5.4",
              ownedByThisFrontend: true,
              owner: "webview",
              pendingAttachments: [],
              planId: null,
              planState: "chat",
              sessionId: "s1",
              timeline: [{ id: `m-${index}`, kind: "assistant", text: "chunk", type: "message" }],
            },
          },
          uiMode: "both",
        },
        messageId: `state-underfill-cap-${index}`,
      });
    }

    const loadOlderCalls = postMessage.mock.calls.filter(
      ([message]) => message?.type === "loadOlderHistory",
    );
    expect(loadOlderCalls).toHaveLength(4);
  });

  it("hides the subtle history loader when loading finishes", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: true,
            historyLoading: true,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-loader-on",
    });

    expect(screen.getByTestId("history-loader")).toBeTruthy();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [{ id: "m1", kind: "assistant", text: "done", type: "message" }],
          },
        },
        uiMode: "both",
      },
      messageId: "state-loader-off",
    });

    expect(screen.queryByTestId("history-loader")).toBeNull();
  });

  it("posts prompt and composer action intents", async () => {
    const { postMessage } = mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4", "claude-4.6-sonnet"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: 0.42,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [
              {
                attachment: {
                  dataBase64: "YWJj",
                  kind: "file",
                  mimeType: "text/markdown",
                },
                id: "att-1",
                kind: "file",
                label: "README.md",
                mimeType: "text/markdown",
                path: "/workspace/README.md",
              },
            ],
            planFile: {
              path: "/workspace/login-refactor.plan.md",
              planId: "plan-1",
              state: "planning",
            },
            planId: null,
            planState: "planning",
            sessionId: "s1",
            timeline: [
              {
                id: "plan-card-1",
                path: "/workspace/login-refactor.plan.md",
                planId: "plan-1",
                state: "planning",
                type: "plan",
              },
              {
                id: "approval-1",
                request: {
                  questions: [
                    {
                      id: "q1",
                      options: [
                        { id: "yes", label: "Yes", recommended: true },
                        { id: "no", label: "No" },
                      ],
                      prompt: "Proceed?",
                    },
                  ],
                  requestId: "r1",
                  responseEvent: "response",
                },
                resolved: false,
                sessionId: "s1",
                type: "approval",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-2",
    });

    fireEvent.paste(screen.getByTestId("composer-input"), {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "send this" : ""),
      },
    });
    fireEvent.click(screen.getByTestId("send-button"));
    const modelSelect = screen.getByTestId("model-select");
    if (modelSelect.tagName === "SELECT") {
      fireEvent.change(modelSelect, {
        target: { value: "claude-4.6-sonnet" },
      });
    } else {
      fireEvent.click(modelSelect);
      fireEvent.click(
        screen
          .getAllByTestId("model-option")
          .find((node) => node.textContent?.includes("claude-4.6-sonnet")) ??
          screen.getAllByTestId("model-option")[0],
      );
    }
    fireEvent.click(screen.getByTestId("thinking-level-select"));
    fireEvent.click(
      screen
        .getAllByTestId("thinking-level-option")
        .find((node) => node.textContent?.includes("Xhigh")) ?? screen.getAllByTestId("thinking-level-option")[0],
    );
    fireEvent.click(screen.getByTestId("mode-select"));
    fireEvent.click(
      screen.getAllByTestId("mode-option").find((node) => node.textContent?.includes("Chat")) ??
        screen.getAllByTestId("mode-option")[0],
    );
    fireEvent.click(screen.getByLabelText("添加文件/文件夹/图片"));
    fireEvent.click(screen.getByTestId("attachment-chip"));
    fireEvent.click(screen.getByTestId("plan-card-title"));
    fireEvent.click(screen.getByTestId("build-plan"));
    fireEvent.click(screen.getByTestId("approval-option-q1-yes"));
    fireEvent.click(screen.getByTestId("approval-continue"));

    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "prompt" && message.data?.text === "send this",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "answerQuestion" &&
          message.data?.requestId === "r1" &&
          message.data?.result?.cancelled === false &&
          message.data?.result?.answers?.[0]?.questionId === "q1" &&
          message.data?.result?.answers?.[0]?.optionIds?.[0] === "yes" &&
          message.data?.result?.answers?.[0]?.pickedRecommended === true,
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "setModel" &&
          message.data?.modelId === "claude-4.6-sonnet",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "setThinkingLevel" &&
          message.data?.level === "xhigh" &&
          message.data?.modelId === "gpt-5.4",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "setPlanMode" &&
          message.data?.action === "exit",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(([message]) => message.type === "pickContext"),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "removeAttachment" &&
          message.data?.attachmentId === "att-1",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "openPlanFile" &&
          message.data?.path === "/workspace/login-refactor.plan.md",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "setPlanMode" &&
          message.data?.action === "build" &&
          message.data?.planId === "plan-1",
      ),
    ).toBe(true);
  });

  it("posts batched approval answers after all questions are selected", async () => {
    const { postMessage } = mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [
              {
                id: "approval-1",
                request: {
                  questions: [
                    {
                      id: "q1",
                      options: [
                        { id: "day", label: "Day", recommended: true },
                        { id: "night", label: "Night" },
                      ],
                      prompt: "When do you prefer to code?",
                    },
                    {
                      id: "q2",
                      options: [
                        { id: "ts", label: "TypeScript", recommended: true },
                        { id: "rs", label: "Rust" },
                      ],
                      prompt: "Which language do you want to use?",
                    },
                  ],
                  requestId: "r2",
                  responseEvent: "response-2",
                },
                resolved: false,
                sessionId: "s1",
                type: "approval",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-batched-approval",
    });

    const continueButton = screen.getByTestId("approval-continue");
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId("approval-option-q1-day"));
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId("approval-option-q2-rs"));
    expect((continueButton as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(continueButton);

    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "answerQuestion" &&
          message.data?.requestId === "r2" &&
          message.data?.result?.cancelled === false &&
          JSON.stringify(message.data?.result?.answers) ===
            JSON.stringify([
              {
                optionIds: ["day"],
                pickedRecommended: true,
                questionId: "q1",
              },
              {
                optionIds: ["rs"],
                pickedRecommended: false,
                questionId: "q2",
              },
            ]),
      ),
    ).toBe(true);
  });

  it("submits the prompt on Enter without Shift", async () => {
    const { postMessage } = mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-enter",
    });

    const textbox = screen.getByTestId("composer-input");
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "submit via enter" : ""),
      },
    });
    fireEvent.keyDown(textbox, { key: "Enter" });

    expect(
      postMessage.mock.calls.some(
        ([message]) =>
          message.type === "prompt" && message.data?.text === "submit via enter",
      ),
    ).toBe(true);
  });

  it("renders a conflict banner for read-only sessions", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "locked-session",
        availableModels: [],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: false,
            owner: "participant",
            sessionId: "locked-session",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          "locked-session": {
            busy: false,
            conflictMessage: "This session is currently owned by the Tomcat participant.",
            contextRatio: null,
            model: null,
            ownedByThisFrontend: false,
            owner: "participant",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "locked-session",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-conflict",
    });

    expect(
      screen.getByText("This session is currently owned by the Tomcat participant."),
    ).toBeTruthy();
  });

  it("renders the top bar without legacy title or refresh button", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "medium",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-topbar",
    });

    expect(screen.getByTestId("new-session-button").textContent).toBe("+");
    expect(screen.getByTestId("connection-chip").getAttribute("aria-label")).toContain(
      "Connected",
    );
    expect(screen.getByTestId("connection-chip").className).toContain(
      "tc-conn-light--connected",
    );
    expect(screen.queryByText("Tomcat")).toBeNull();
    expect(screen.queryByRole("button", { name: /refresh/i })).toBeNull();

    const topbar = screen.getByLabelText("Session bar");
    expect(topbar.firstElementChild).toBe(screen.getByTestId("connection-chip"));
    expect(topbar.lastElementChild).toBe(screen.getByTestId("new-session-button"));
  });

  it("updates the thinking level select from session state", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4", "claude-4.6-sonnet"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-thinking-gpt",
    });

    expect(screen.getByTestId("thinking-level-select").textContent).toContain("High");

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4", "claude-4.6-sonnet"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 2,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "claude-4.6-sonnet",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "low",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-thinking-claude",
    });

    expect(screen.getByTestId("thinking-level-select").textContent).toContain("Low");
  });

  it("shows a sticky prompt and live cluster for the active turn", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: true,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: true,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [
              {
                id: "user-1",
                kind: "user",
                text: "今天美国那边有什么有趣的新闻",
                type: "message",
              },
              {
                id: "thinking-1",
                text: "先整理美国热点新闻，再决定是否需要补 fetch。",
                type: "thinking",
              },
              {
                id: "tool-1",
                isError: false,
                status: "complete",
                summary: "已完成第一轮搜索",
                toolCallId: "tool-1",
                toolName: "web_search",
                type: "tool",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-live-cluster",
    });

    expect(screen.getByTestId("live-cluster")).toBeTruthy();
    expect(screen.getByTestId("thinking-summary").textContent).toContain("先整理美国热点新闻");
    expect(
      document.querySelector(".tc-thinking pre")?.textContent,
    ).toBeFalsy();

    mockScrollableTranscript({
      scrollHeight: 320,
      scrollTop: 220,
      userBottom: -160,
      userTop: -200,
    });
    fireEvent.scroll(screen.getByTestId("stream-container"));

    expect(screen.getByTestId("sticky-user-prompt-text").textContent).toContain(
      "今天美国那边有什么有趣的新闻",
    );

    fireEvent.click(screen.getByTestId("thinking-toggle"));
    expect(document.querySelector(".tc-thinking pre")?.textContent).toContain(
      "先整理美国热点新闻，再决定是否需要补 fetch。",
    );
  });

  it("drives send stop state from busy instead of inferring from transcript tail", async () => {
    mount();

    const danglingTimeline = [
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1-thinking",
        summaryTitle: null,
        text: "stale thinking",
        type: "thinking" as const,
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant" as const,
        text: "previous answer",
        type: "message" as const,
      },
      {
        id: "warn-1",
        kind: "warn" as const,
        text: "Tomcat turn interrupted",
        type: "message" as const,
      },
    ];

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: true,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: "Busy session",
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: true,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: danglingTimeline,
          },
        },
        uiMode: "both",
      },
      messageId: "state-busy-tail",
    });

    expect(screen.getByTestId("stop-button")).toBeTruthy();
    expect(screen.queryByTestId("send-button")).toBeNull();
    expect(screen.getByTestId("session-select").textContent).toContain("running");

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: "Busy session",
            updatedAt: 2,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: danglingTimeline,
          },
        },
        uiMode: "both",
      },
      messageId: "state-idle-tail",
    });

    expect(screen.getByTestId("send-button")).toBeTruthy();
    expect(screen.queryByTestId("stop-button")).toBeNull();
    expect(screen.getByTestId("session-select").textContent).not.toContain("running");
  });

  it("shows the session title instead of the raw sessionId in the dropdown", async () => {
    mount();
    const now = Date.now();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "1781621492962_3ee132361e6832e6",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "1781621492962_3ee132361e6832e6",
            title: "帮我重构 session 列表",
            updatedAt: now,
          },
        ],
        sessionViews: {
          "1781621492962_3ee132361e6832e6": {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "1781621492962_3ee132361e6832e6",
            thinkingLevel: "medium",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-title",
    });

    expect(screen.getByTestId("session-select").textContent).toContain(
      "帮我重构 session 列表",
    );
    expect(screen.getByTestId("session-select").textContent).not.toContain(
      "1781621492962",
    );

    fireEvent.click(screen.getByTestId("session-select"));
    expect(screen.getByTestId("session-option").textContent).toContain(
      "帮我重构 session 列表",
    );
  });

  it("falls back to New session when title is empty or whitespace", async () => {
    mount();
    const now = Date.now();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "empty-session",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "empty-session",
            title: null,
            updatedAt: now,
          },
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "whitespace-session",
            title: "   ",
            updatedAt: now - 1000,
          },
        ],
        sessionViews: {
          "empty-session": {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "empty-session",
            thinkingLevel: "medium",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-empty-title",
    });

    expect(screen.getByTestId("session-select").textContent).toContain("New session");
    expect(screen.getByTestId("session-select").textContent).not.toContain("empty-session");

    fireEvent.click(screen.getByTestId("session-select"));
    const options = screen.getAllByTestId("session-option").map((o) => o.textContent);
    expect(options[0]).toContain("New session");
    expect(options[1]).toContain("New session");
    expect(options.every((o) => !o?.includes("whitespace-session"))).toBe(true);
  });

  it("caps each group at 6 and reveals more on click", async () => {
    mount();
    const now = Date.now();
    const sessions = Array.from({ length: 7 }, (_, index) => ({
      busy: false,
      isCurrent: index === 0,
      ownedByThisFrontend: true,
      owner: "webview" as const,
      sessionId: `s${index}`,
      title: `topic ${index}`,
      updatedAt: now - index * 1000,
    }));

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s0",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions,
        sessionViews: {
          s0: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s0",
            thinkingLevel: "medium",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-cap",
    });

    fireEvent.click(screen.getByTestId("session-select"));
    expect(screen.getAllByTestId("session-option").length).toBe(6);
    expect(screen.getByTestId("session-more").textContent).toContain("Show 1 more");

    fireEvent.click(screen.getByTestId("session-more"));
    expect(screen.getAllByTestId("session-option").length).toBe(7);
    expect(screen.queryByTestId("session-more")).toBeNull();
  });

  it("groups sessions by date with section headers", async () => {
    mount();
    const now = Date.now();
    const day = 24 * 60 * 60 * 1000;

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "today-s",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "today-s",
            title: "today topic",
            updatedAt: now - 60_000,
          },
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "yesterday-s",
            title: "yesterday topic",
            updatedAt: now - day - 60_000,
          },
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "last7-s",
            title: "last7 topic",
            updatedAt: now - 4 * day,
          },
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "last30-s",
            title: "last30 topic",
            updatedAt: now - 20 * day,
          },
          {
            busy: false,
            isCurrent: false,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "older-s",
            title: "older topic",
            updatedAt: now - 400 * day,
          },
        ],
        sessionViews: {
          "today-s": {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "today-s",
            thinkingLevel: "medium",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-groups",
    });

    fireEvent.click(screen.getByTestId("session-select"));
    const headers = screen.getAllByTestId("session-group-header").map((h) => h.textContent);
    expect(headers).toEqual([
      "Today",
      "Yesterday",
      "Last 7 days",
      "Last 30 days",
      "Older",
    ]);
  });

  it("accepts insertReference events and sends reference-only prompts", async () => {
    const { postMessage } = mount();

    await emitReadySessionState("s1");

    await emitState({
      channel: "event",
      content: {
        reference: {
          kind: "file",
          label: "app.ts",
          path: "src/app.ts",
          type: "reference",
        },
        sessionId: "s1",
        type: "insertReference",
      },
      messageId: "event-insert-reference",
    });

    expect(screen.getByTestId("composer-reference-chip").textContent).toContain("app.ts");

    fireEvent.click(screen.getByTestId("send-button"));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          segments: [
            {
              kind: "file",
              label: "app.ts",
              lineEnd: null,
              lineStart: null,
              path: "src/app.ts",
              text: null,
              type: "reference",
            },
            {
              text: " ",
              type: "text",
            },
          ],
          sessionId: "s1",
          text: "app.ts ",
          userMessageId: expect.any(String),
        }),
        type: "prompt",
      }),
    );
  });

  it("keeps composer content until a sent prompt is confirmed, then clears it", async () => {
    const { postMessage } = mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-clear-success",
    });

    const textbox = screen.getByTestId("composer-input");
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "send this" : ""),
      },
    });
    fireEvent.click(screen.getByTestId("send-button"));

    expect(textbox.textContent).toContain("send this");

    const promptMessage = postMessage.mock.calls.find(([message]) => message.type === "prompt")?.[0];
    const userMessageId = promptMessage?.data?.userMessageId;
    expect(typeof userMessageId).toBe("string");

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 2,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [
              {
                id: userMessageId,
                kind: "user",
                text: "send this",
                type: "message",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-clear-success-confirmed",
    });

    expect(screen.getByTestId("composer-input").textContent?.trim()).toBe("");
  });

  it("keeps composer content when a sent prompt fails", async () => {
    const { postMessage } = mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-clear-failed",
    });

    const textbox = screen.getByTestId("composer-input");
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "keep me" : ""),
      },
    });
    fireEvent.click(screen.getByTestId("send-button"));

    const promptMessage = postMessage.mock.calls.find(([message]) => message.type === "prompt")?.[0];
    const userMessageId = promptMessage?.data?.userMessageId;
    expect(typeof userMessageId).toBe("string");

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 2,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [
              {
                deliveryError: "busy",
                deliveryState: "failed",
                id: userMessageId,
                kind: "user",
                retryable: true,
                text: "keep me",
                type: "message",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-clear-failed-result",
    });

    expect(screen.getByTestId("composer-input").textContent).toContain("keep me");
  });

  it("preserves composer content when the session flips to read-only", async () => {
    mount();

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 1,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: null,
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-readonly-before",
    });

    const textbox = screen.getByTestId("composer-input");
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "keep this draft" : ""),
      },
    });
    expect(textbox.textContent).toContain("keep this draft");

    await emitState({
      channel: "state",
      content: {
        activeSessionId: "s1",
        availableModels: ["gpt-5.4"],
        ready: true,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            ownedByThisFrontend: true,
            owner: "webview",
            sessionId: "s1",
            title: null,
            updatedAt: 2,
          },
        ],
        sessionViews: {
          s1: {
            busy: false,
            conflictMessage: "This live session is currently read-only in the webview.",
            contextRatio: null,
            hasMoreHistory: false,
            historyLoading: false,
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            pendingAttachments: [],
            planFile: null,
            planId: null,
            planState: "chat",
            sessionId: "s1",
            thinkingLevel: "high",
            timeline: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-readonly-after",
    });

    expect(screen.getByTestId("conflict-banner").textContent).toContain("read-only");
    expect(screen.getByTestId("composer-input").textContent).toContain("keep this draft");
  });

  it("ignores malformed insertReference events without a concrete session id", async () => {
    mount();

    await emitReadySessionState("s1");

    await emitState({
      channel: "event",
      content: {
        reference: {
          kind: "file",
          label: "app.ts",
          path: "src/app.ts",
          type: "reference",
        },
        sessionId: null,
        type: "insertReference",
      } as HostToWebviewFrame["content"],
      messageId: "event-invalid-insert-reference",
    });

    expect(screen.queryByTestId("composer-reference-chip")).toBeNull();
  });

  it("debounces @ searches, routes fresh results, and drops stale ones", async () => {
    vi.useFakeTimers();
    try {
      const { postMessage } = mount();
      await emitReadySessionState("s1");
      postMessage.mockClear();

      const textbox = screen.getByTestId("composer-input");
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "@app" : ""),
          },
        });
      });

      expect(screen.getByTestId("context-search-loading").textContent).toContain("搜索中");
      expect(postMessage).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(150);
      });

      const searchIntent = postMessage.mock.calls.find(
        ([message]) => message.type === "searchContext",
      )?.[0];
      expect(searchIntent?.data).toMatchObject({
        kind: "file",
        query: "app",
        sessionId: "s1",
      });

      await emitState({
        channel: "event",
        content: {
          matches: [
            {
              description: "old",
              reference: {
                kind: "file",
                label: "old.ts",
                path: "old/old.ts",
                type: "reference",
              },
            },
          ],
          query: "app",
          requestId: "stale-request",
          truncated: false,
          type: "contextSearchResult",
          workspaceAvailable: true,
        },
        messageId: "stale-result",
      });
      expect(screen.queryByText("old.ts")).toBeNull();

      await emitState({
        channel: "event",
        content: {
          matches: [
            {
              description: "src",
              reference: {
                kind: "file",
                label: "app.ts",
                path: "src/app.ts",
                type: "reference",
              },
            },
          ],
          query: "app",
          requestId: searchIntent?.data?.requestId ?? "missing",
          truncated: false,
          type: "contextSearchResult",
          workspaceAvailable: true,
        },
        messageId: "fresh-result",
      });

      expect(screen.getByTestId("context-search-dropdown")).toBeTruthy();
      expect(screen.getByTitle("src/app.ts")).toBeTruthy();
      expect(screen.getByText("src")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("warns once and closes the menu when @ search runs without a workspace", async () => {
    vi.useFakeTimers();
    try {
      const { postMessage } = mount();
      await emitReadySessionState("s1");
      postMessage.mockClear();

      const textbox = screen.getByTestId("composer-input");
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "@app" : ""),
          },
        });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(150);
      });
      const firstSearch = postMessage.mock.calls.find(
        ([message]) => message.type === "searchContext",
      )?.[0];
      expect(firstSearch?.data?.requestId).toBeTruthy();

      await emitState({
        channel: "event",
        content: {
          matches: [],
          query: "app",
          requestId: firstSearch?.data?.requestId ?? "missing",
          truncated: false,
          type: "contextSearchResult",
          workspaceAvailable: false,
        },
        messageId: "no-workspace-result-1",
      });

      expect(
        postMessage.mock.calls.filter(([message]) => message.type === "showWarningMessage"),
      ).toHaveLength(1);
      expect(screen.queryByTestId("context-search-dropdown")).toBeNull();

      postMessage.mockClear();
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "@app" : ""),
          },
        });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(150);
      });
      const secondSearch = postMessage.mock.calls.find(
        ([message]) => message.type === "searchContext",
      )?.[0];
      await emitState({
        channel: "event",
        content: {
          matches: [],
          query: "app",
          requestId: secondSearch?.data?.requestId ?? "missing-2",
          truncated: false,
          type: "contextSearchResult",
          workspaceAvailable: false,
        },
        messageId: "no-workspace-result-2",
      });

      expect(
        postMessage.mock.calls.filter(([message]) => message.type === "showWarningMessage"),
      ).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the previous @ results visible while the next query is debouncing", async () => {
    vi.useFakeTimers();
    try {
      const { postMessage } = mount();
      await emitReadySessionState("s1");
      postMessage.mockClear();

      const textbox = screen.getByTestId("composer-input");
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "@app" : ""),
          },
        });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(150);
      });

      const searchIntent = postMessage.mock.calls.find(
        ([message]) => message.type === "searchContext",
      )?.[0];
      await emitState({
        channel: "event",
        content: {
          matches: [
            {
              description: "src",
              reference: {
                kind: "file",
                label: "app.ts",
                path: "src/app.ts",
                type: "reference",
              },
            },
          ],
          query: "app",
          requestId: searchIntent?.data?.requestId ?? "missing",
          truncated: false,
          type: "contextSearchResult",
          workspaceAvailable: true,
        },
        messageId: "context-search-result-app",
      });

      postMessage.mockClear();
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "l" : ""),
          },
        });
      });

      expect(screen.getByTitle("src/app.ts")).toBeTruthy();
      expect(screen.getByTestId("context-search-loading-inline").textContent).toContain("搜索中");
      expect(postMessage).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("closes an active @ dropdown when the active session changes", async () => {
    const { postMessage } = mount();
    await emitReadySessionState("s1");
    postMessage.mockClear();

    const textbox = screen.getByTestId("composer-input");
    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app" : ""),
        },
      });
    });

    expect(screen.getByTestId("context-search-dropdown")).toBeTruthy();

    await emitReadySessionState("s2");

    expect(screen.queryByTestId("context-search-dropdown")).toBeNull();
  });
});
