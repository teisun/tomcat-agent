import { describe, expect, it } from "vitest";

import { WebviewStateStore } from "../../src/ui/webview/state";
import { mergeSessionViewSnapshot } from "./stateReconcile";
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
        activePlan: null,
        agentMode: "chat",
        busy: false,
        checkpoints: [],
        contextRatio: null,
        hasMoreHistory: false,
        historyLoading: false,
        model: "gpt-5.4",
        ownedByThisFrontend: true,
        pendingAttachments: [],
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

  it("keeps a live ask_question approval after its tool is interrupted", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    let rendered = store.snapshot();

    const applyLivePatch = () => {
      const mutation = store.applyEvent({
        args: { questions: [] },
        sessionId: "s1",
        toolCallId: "ask-call-1",
        toolName: "ask_question",
        type: "tool_execution_start",
      });
      expect(mutation.kind).toBe("patch");
      if (mutation.kind !== "patch") {
        throw new Error("tool_execution_start must produce a patch");
      }
      const result = applySessionPatchFrame(rendered, mutation);
      expect(result.ok).toBe(true);
      if (!result.ok) {
        throw new Error(result.error);
      }
      rendered = result.state;
    };

    applyLivePatch();
    const approvalMutation = store.applyEvent({
      payload: {
        questions: [{
          id: "q1",
          options: [{ id: "yes", label: "Yes", recommended: true }],
          prompt: "Proceed?",
        }],
        requestId: "request-1",
        responseEvent: "plan.ask_question.response.request-1",
        sessionId: "s1",
        toolCallId: "ask-call-1",
      },
      requestId: "request-1",
      sessionId: "s1",
      subtype: "ask_question",
      type: "control_request",
    });
    expect(approvalMutation.kind).toBe("session");
    if (approvalMutation.kind !== "session") {
      throw new Error("control_request must refresh the session view");
    }
    rendered = store.snapshot();
    expect(new Set(rendered.sessionViews.s1.timeline.map((item) => item.id)).size)
      .toBe(rendered.sessionViews.s1.timeline.length);

    const interruptMutation = store.applyEvent({
      isError: false,
      result: "[interrupted]",
      sessionId: "s1",
      toolCallId: "ask-call-1",
      toolName: "ask_question",
      type: "tool_execution_end",
    });
    expect(interruptMutation.kind).toBe("patch");
    if (interruptMutation.kind !== "patch") {
      throw new Error("tool_execution_end must produce a patch");
    }
    const result = applySessionPatchFrame(rendered, interruptMutation);
    expect(result.ok).toBe(true);
    if (!result.ok) {
      throw new Error(result.error);
    }

    const items = result.state.sessionViews.s1.timeline;
    expect(items.filter((item) => item.type === "tool" && item.toolCallId === "ask-call-1"))
      .toHaveLength(1);
    expect(items.filter((item) => item.type === "approval" && item.request.toolCallId === "ask-call-1"))
      .toHaveLength(1);
  });

  it("keeps the ask_question timeline equivalent for patch and full-session delivery", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    let patchState = store.snapshot();
    let fullState = store.snapshot();

    const applyFullSession = () => {
      const view = store.snapshotSession("s1");
      if (!view) {
        throw new Error("expected active session view");
      }
      fullState = mergeSessionViewSnapshot(fullState, {
        sessionId: "s1",
        view,
      });
    };
    const applyPatch = () => {
      const mutation = store.applyEvent({
        args: { questions: [] },
        sessionId: "s1",
        toolCallId: "ask-call-equivalent",
        toolName: "ask_question",
        type: "tool_execution_start",
      });
      expect(mutation.kind).toBe("patch");
      if (mutation.kind !== "patch") {
        throw new Error("tool_execution_start must produce a patch");
      }
      const result = applySessionPatchFrame(patchState, mutation);
      expect(result.ok).toBe(true);
      if (!result.ok) {
        throw new Error(result.error);
      }
      patchState = result.state;
      applyFullSession();
    };

    applyPatch();
    store.applyEvent({
      payload: {
        questions: [{
          id: "q1",
          options: [{ id: "yes", label: "Yes", recommended: true }],
          prompt: "Proceed?",
        }],
        requestId: "request-equivalent",
        responseEvent: "plan.ask_question.response.request-equivalent",
        sessionId: "s1",
        toolCallId: "ask-call-equivalent",
      },
      requestId: "request-equivalent",
      sessionId: "s1",
      subtype: "ask_question",
      type: "control_request",
    });
    patchState = store.snapshot();
    applyFullSession();

    const endMutation = store.applyEvent({
      isError: false,
      result: "[interrupted]",
      sessionId: "s1",
      toolCallId: "ask-call-equivalent",
      toolName: "ask_question",
      type: "tool_execution_end",
    });
    expect(endMutation.kind).toBe("patch");
    if (endMutation.kind !== "patch") {
      throw new Error("tool_execution_end must produce a patch");
    }
    const patched = applySessionPatchFrame(patchState, endMutation);
    expect(patched.ok).toBe(true);
    if (!patched.ok) {
      throw new Error(patched.error);
    }
    patchState = patched.state;
    applyFullSession();

    expect(patchState.sessionViews.s1.timeline).toEqual(fullState.sessionViews.s1.timeline);
  });
});
