import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { HostToWebviewFrame, VsCodeApiLike, WebviewStateSnapshot } from "./types";

function mount() {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike = {
    postMessage,
    setState: vi.fn(),
  };
  render(<App vscodeApi={vscodeApi} />);
  return { postMessage };
}

async function emitFrame(frame: HostToWebviewFrame) {
  await act(async () => {
    window.dispatchEvent(new MessageEvent("message", { data: frame }));
  });
}

function baseState(): WebviewStateSnapshot {
  return {
    activeSessionId: "s1",
    availableModelCapabilities: {},
    availableModelReasoningLevels: {},
    availableModels: ["gpt-5.4"],
    modelAdminSupported: false,
    ready: true,
    sessions: [
      {
        busy: false,
        isCurrent: true,
        ownedByThisFrontend: true,
        sessionId: "s1",
        title: "Session 1",
        updatedAt: 1,
      },
    ],
    sessionViews: {
      s1: {
        busy: false,
        checkpoints: [],
        contextRatio: null,
        hasMoreHistory: false,
        historyLoading: false,
        model: "gpt-5.4",
        ownedByThisFrontend: true,
        pendingAttachments: [],
        planFile: null,
        planId: null,
        planState: "chat",
        planTodos: [],
        sessionId: "s1",
        sessionTodos: [],
        thinkingLevel: "high",
        timeline: [
          {
            assistantMessageId: "assistant-1",
            id: "assistant-1",
            kind: "assistant",
            text: "hel",
            type: "message",
          },
        ],
      },
    },
  };
}

describe("App session frames", () => {
  it("applies sessionPatch frames to the active transcript", async () => {
    mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    expect(screen.getByText("hel")).toBeTruthy();

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });

    expect(screen.getByText("hello")).toBeTruthy();
  });

  it("requests a session resync when patch seqs skip ahead", async () => {
    const { postMessage } = mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });
    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: " world", type: "appendText" }],
        seq: 3,
        sessionId: "s1",
      },
      messageId: "patch-gap",
    });

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { sessionId: "s1" },
        type: "resyncSessionView",
      }),
    );
  });

  it("clears resync-pending state when a sessionView arrives", async () => {
    const { postMessage } = mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });
    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: " world", type: "appendText" }],
        seq: 3,
        sessionId: "s1",
      },
      messageId: "patch-gap",
    });

    await emitFrame({
      channel: "sessionView",
      content: {
        sessionId: "s1",
        tab: {
          busy: false,
          isCurrent: true,
          ownedByThisFrontend: true,
          sessionId: "s1",
          title: "Session 1",
          updatedAt: 2,
        },
        view: {
          ...baseState().sessionViews.s1,
          timeline: [
            {
              assistantMessageId: "assistant-1",
              id: "assistant-1",
              kind: "assistant",
              text: "hello world",
              type: "message",
            },
          ],
        },
      },
      messageId: "session-view-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "!", type: "appendText" }],
        seq: 4,
        sessionId: "s1",
      },
      messageId: "patch-after-resync",
    });

    expect(screen.getByText("hello world!")).toBeTruthy();
    expect(
      postMessage.mock.calls.filter(
        ([message]) => (message as { type?: string }).type === "resyncSessionView",
      ),
    ).toHaveLength(1);
  });
});
