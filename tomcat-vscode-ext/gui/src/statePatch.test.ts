import { describe, expect, it } from "vitest";

import { applySessionPatchFrame } from "./statePatch";
import type { WebviewStateSnapshot } from "./types";

function snapshot(): WebviewStateSnapshot {
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
        title: "s1",
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
            id: "user-1",
            kind: "user",
            text: "prompt",
            type: "message",
          },
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

describe("statePatch", () => {
  it("applies appendText while preserving unrelated item references", () => {
    const previous = snapshot();
    const stableUser = previous.sessionViews.s1.timeline[0];

    const result = applySessionPatchFrame(previous, {
      ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
      sessionId: "s1",
    });

    expect(result.ok).toBe(true);
    if (!result.ok) {
      return;
    }
    expect(result.state.sessionViews.s1.timeline[0]).toBe(stableUser);
    expect(result.state.sessionViews.s1.timeline[1]).not.toBe(
      previous.sessionViews.s1.timeline[1],
    );
    expect(result.state.sessionViews.s1.timeline[1]).toMatchObject({
      id: "assistant-1",
      text: "hello",
    });
  });

  it("inserts new items using beforeId positioning", () => {
    const previous = snapshot();

    const result = applySessionPatchFrame(previous, {
      ops: [
        {
          beforeId: "assistant-1",
          item: {
            assistantMessageId: "assistant-1",
            id: "assistant-1-thinking",
            summaryTitle: null,
            text: "thinking",
            type: "thinking",
          },
          type: "upsert",
        },
      ],
      sessionId: "s1",
    });

    expect(result.ok).toBe(true);
    if (!result.ok) {
      return;
    }
    expect(result.state.sessionViews.s1.timeline.map((item) => item.id)).toEqual([
      "user-1",
      "assistant-1-thinking",
      "assistant-1",
    ]);
  });

  it("returns an error when a patch references a missing item", () => {
    const result = applySessionPatchFrame(snapshot(), {
      ops: [{ id: "missing", text: "oops", type: "appendText" }],
      sessionId: "s1",
    });

    expect(result).toEqual({
      error: "missing item missing",
      ok: false,
    });
  });
});
