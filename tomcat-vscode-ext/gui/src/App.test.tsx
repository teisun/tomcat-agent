import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

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

describe("Tomcat webview App", () => {
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
            planId: "plan-1",
            planState: "planning",
            sessionId: "s1",
            timeline: [
              { id: "m1", kind: "assistant", text: "hello", type: "message" },
              { id: "m2", text: "thinking...", type: "thinking" },
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
                      options: [{ id: "yes", label: "Yes", recommended: true }],
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
    expect(screen.queryByText("thinking...")).toBeNull();
    fireEvent.click(screen.getByTestId("thinking-toggle"));
    expect(screen.getByText("thinking...")).toBeTruthy();
    expect(screen.getByText("Proceed?")).toBeTruthy();
    expect(screen.getByText("edit (complete)")).toBeTruthy();
    expect(screen.getByTestId("session-option").textContent).toContain("s1");
    expect(screen.getByTestId("plan-card").textContent).toContain("login-refactor.plan.md");
    expect(screen.getByTestId("build-plan").textContent).toContain("Build");
    expect(screen.getByTestId("attachment-chip").textContent).toContain("README.md");
    expect(screen.getByTestId("context-ratio").textContent).toContain("Ctx 42%");
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
                      options: [{ id: "yes", label: "Yes", recommended: true }],
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

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "send this" },
    });
    fireEvent.click(screen.getByTestId("send-button"));
    fireEvent.change(screen.getByTestId("model-select"), {
      target: { value: "claude-4.6-sonnet" },
    });
    fireEvent.change(screen.getByTestId("mode-select"), {
      target: { value: "chat" },
    });
    fireEvent.click(screen.getByLabelText("Add attachment"));
    fireEvent.click(screen.getByTestId("attachment-chip"));
    fireEvent.click(screen.getByLabelText("Open plan file"));
    fireEvent.click(screen.getByTestId("build-plan"));
    fireEvent.click(screen.getByText("Yes (Recommended)"));

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
          message.data?.requestId === "r1",
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
          message.type === "setPlanMode" &&
          message.data?.action === "exit",
      ),
    ).toBe(true);
    expect(
      postMessage.mock.calls.some(([message]) => message.type === "pickAttachment"),
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
          message.data?.action === "build",
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

    const textbox = screen.getByRole("textbox");
    fireEvent.change(textbox, {
      target: { value: "submit via enter" },
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
});
