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
  it("renders messages, tools, approvals, and session tabs from state", async () => {
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
            approvals: [
              {
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
              },
            ],
            busy: false,
            conflictMessage: null,
            messages: [
              { id: "m1", kind: "assistant", text: "hello" },
              { id: "m2", kind: "thinking", text: "thinking..." },
            ],
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            planId: "plan-1",
            planState: "planning",
            sessionId: "s1",
            tools: [
              {
                display: { file: "src/app.ts", kind: "file" },
                isError: false,
                status: "complete",
                summary: "updated file",
                toolCallId: "tool-1",
                toolName: "edit",
              },
            ],
          },
        },
        uiMode: "both",
      },
      messageId: "state-1",
    });

    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.getByText("thinking...")).toBeTruthy();
    expect(screen.getByText("Proceed?")).toBeTruthy();
    expect(screen.getByText("edit (complete)")).toBeTruthy();
    expect(screen.getByText("s1 * (webview)")).toBeTruthy();
  });

  it("posts prompt and approval intents", async () => {
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
            approvals: [
              {
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
              },
            ],
            busy: false,
            conflictMessage: null,
            messages: [],
            model: "gpt-5.4",
            ownedByThisFrontend: true,
            owner: "webview",
            planId: null,
            planState: "chat",
            sessionId: "s1",
            tools: [],
          },
        },
        uiMode: "both",
      },
      messageId: "state-2",
    });

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "send this" },
    });
    fireEvent.click(screen.getByText("Send"));
    fireEvent.click(screen.getByText("Yes"));

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
            approvals: [],
            busy: false,
            conflictMessage: "This session is currently owned by the Tomcat participant.",
            messages: [],
            model: null,
            ownedByThisFrontend: false,
            owner: "participant",
            planId: null,
            planState: "chat",
            sessionId: "locked-session",
            tools: [],
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
