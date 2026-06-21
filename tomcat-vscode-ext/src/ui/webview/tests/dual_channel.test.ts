import { describe, expect, it } from "vitest";

import { WebviewStateStore } from "../state";

describe("webview dual-channel state store", () => {
  it("maps state snapshots and passthrough events into rich UI state", () => {
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

    const snapshot = store.snapshot();
    expect(snapshot.activeSessionId).toBe("s1");
    expect(snapshot.sessions[0]).toMatchObject({
      ownedByThisFrontend: true,
      owner: "webview",
      sessionId: "s1",
    });
    expect(snapshot.sessionViews.s1.messages).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "assistant", text: "hello" }),
        expect.objectContaining({ kind: "thinking", text: "thinking" }),
      ]),
    );
    expect(snapshot.sessionViews.s1.tools[0]).toMatchObject({
      display: { file: "src/app.ts", kind: "file" },
      status: "complete",
      toolCallId: "tool-1",
    });
    expect(snapshot.sessionViews.s1.approvals[0]?.request.requestId).toBe("ask-1");
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
      planId: "plan-1",
      planState: "executing",
    });
  });
});
