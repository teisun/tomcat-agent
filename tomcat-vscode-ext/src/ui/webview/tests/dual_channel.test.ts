import { describe, expect, it } from "vitest";

import { WebviewStateStore } from "../state";

describe("webview dual-channel state store", () => {
  it("maps state snapshots, history, and live events into timeline state", () => {
    const store = new WebviewStateStore("both");

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [
          {
            busy: false,
            isCurrent: true,
            sessionId: "s1",
            updatedAt: 123,
          },
        ],
      },
      new Map([["s1", "webview"]]),
      "webview",
    );
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
      "webview",
      "webview",
    );
    store.hydrateHistory("s1", {
      messages: [
        {
          id: "hist-user-1",
          message: {
            content: "older prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "hist-assistant-1",
          message: {
            content: "older answer",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });
    store.setPendingAttachments("s1", [
      {
        attachment: {
          dataBase64: "YWJj",
          kind: "file",
          mimeType: "text/plain",
        },
        id: "att-1",
        kind: "file",
        label: "README.md",
        mimeType: "text/plain",
        path: "/workspace/README.md",
      },
    ]);

    store.applyEvent({
      assistantMessageEvent: {
        delta: "hello",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageEvent: {
        delta: "thinking",
        kind: "thinking_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      compactionCount: 0,
      compactionTokensFreed: 0,
      contextUtilizationRatio: 0.5,
      inputTokensUsed: 128,
      preheatInProgress: false,
      preheatResultPending: false,
      totalToolResultBytesPersisted: 0,
      type: "context_metrics_update",
    });
    store.applyEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "edit",
      type: "tool_execution_start",
    });
    store.applyEvent({
      display: { file: "src/app.ts", kind: "file" },
      isError: false,
      result: { ok: true },
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "edit",
      type: "tool_execution_end",
    });
    store.applyEvent({
      payload: {
        questions: [
          {
            id: "q1",
            options: [{ id: "yes", label: "Yes" }],
            prompt: "Proceed?",
          },
        ],
        requestId: "ask-1",
        responseEvent: "response",
      },
      requestId: "ask-1",
      sessionId: "s1",
      subtype: "ask_question",
      type: "control_request",
    });
    store.applyEvent({
      path: "/workspace/login.plan.md",
      planId: "plan-1",
      sessionId: "s1",
      state: "planning",
      type: "plan.create",
    });

    const snapshot = store.snapshot();
    expect(snapshot.activeSessionId).toBe("s1");
    expect(snapshot.sessions[0]).toMatchObject({
      ownedByThisFrontend: true,
      owner: "webview",
      sessionId: "s1",
    });
    expect(snapshot.sessionViews.s1.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "older prompt", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "older answer", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "hello", type: "message" }),
        expect.objectContaining({ text: "thinking", type: "thinking" }),
        expect.objectContaining({
          display: { file: "src/app.ts", kind: "file" },
          status: "complete",
          toolCallId: "tool-1",
          type: "tool",
        }),
        expect.objectContaining({
          path: "/workspace/login.plan.md",
          planId: "plan-1",
          state: "planning",
          type: "plan",
        }),
        expect.objectContaining({
          request: expect.objectContaining({ requestId: "ask-1" }),
          type: "approval",
        }),
      ]),
    );
    expect(snapshot.sessionViews.s1.planFile).toMatchObject({
      path: "/workspace/login.plan.md",
      planId: "plan-1",
      state: "planning",
    });
    expect(snapshot.sessionViews.s1.contextRatio).toBe(0.5);
    expect(snapshot.sessionViews.s1.pendingAttachments[0]).toMatchObject({
      id: "att-1",
      label: "README.md",
    });
  });

  it("replaces session metadata idempotently from fresh state snapshots", () => {
    const store = new WebviewStateStore("both");

    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
      null,
      "webview",
    );
    store.applyEvent({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      sessionId: "s1",
      state: "planning",
      type: "plan.create",
    });
    store.setPendingAttachments("s1", [
      {
        attachment: {
          dataBase64: "YWJj",
          kind: "file",
          mimeType: "text/plain",
        },
        id: "att-1",
        kind: "file",
        label: "README.md",
        mimeType: "text/plain",
        path: "/workspace/README.md",
      },
    ]);
    store.removePendingAttachment("s1", "att-1");
    store.applySessionState(
      {
        busy: true,
        model: "claude-4.6-sonnet",
        planId: "plan-1",
        planState: "executing",
        sessionId: "s1",
      },
      "webview",
      "webview",
    );

    expect(store.snapshot().sessionViews.s1).toMatchObject({
      busy: true,
      model: "claude-4.6-sonnet",
      planFile: {
        path: "/workspace/plan-a.plan.md",
        planId: "plan-1",
        state: "executing",
      },
      planId: "plan-1",
      planState: "executing",
    });
    expect(store.snapshot().sessionViews.s1.pendingAttachments).toHaveLength(0);
  });
});
