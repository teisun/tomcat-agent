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
            title: null,
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
            thinking_text: "historic thinking",
            tool_calls: [
              {
                function: { name: "load_skill" },
                id: "hist-tool-1",
              },
            ],
          },
          type: "message",
        },
        {
          id: "hist-tool-1-msg",
          message: {
            content: "<skill name=\"repo-archaeology-demo\" />",
            role: "tool",
            tool_call_id: "hist-tool-1",
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
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: {
        delta: "hello",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "live-assistant-1",
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
    const timeline = snapshot.sessionViews.s1.timeline;
    const historicThinkingIndex = timeline.findIndex(
      (item) => item.type === "thinking" && item.id === "hist-assistant-1-thinking",
    );
    const historicAssistantIndex = timeline.findIndex(
      (item) => item.type === "message" && item.id === "hist-assistant-1",
    );
    const liveThinkingIndex = timeline.findIndex(
      (item) => item.type === "thinking" && item.text === "thinking",
    );
    const liveAssistantIndex = timeline.findIndex(
      (item) => item.type === "message" && item.kind === "assistant" && item.text === "hello",
    );
    expect(snapshot.activeSessionId).toBe("s1");
    expect(snapshot.sessions[0]).toMatchObject({
      ownedByThisFrontend: true,
      owner: "webview",
      sessionId: "s1",
    });
    expect(snapshot.sessionViews.s1.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "older prompt", type: "message" }),
        expect.objectContaining({ id: "hist-assistant-1-thinking", text: "historic thinking", type: "thinking" }),
        expect.objectContaining({ kind: "assistant", text: "older answer", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "hello", type: "message" }),
        expect.objectContaining({ text: "thinking", type: "thinking" }),
        expect.objectContaining({
          status: "complete",
          summary: "<skill name=\"repo-archaeology-demo\" />",
          toolCallId: "hist-tool-1",
          toolName: "load_skill",
          type: "tool",
        }),
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
    expect(historicThinkingIndex).toBeGreaterThanOrEqual(0);
    expect(historicAssistantIndex).toBeGreaterThan(historicThinkingIndex);
    expect(liveThinkingIndex).toBeGreaterThanOrEqual(0);
    expect(liveAssistantIndex).toBeGreaterThan(liveThinkingIndex);
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

  it("deduplicates live assistant, thinking, and tool entries when history rehydrates", () => {
    const store = new WebviewStateStore("both");

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
      new Map([["s1", "webview"]]),
      "webview",
    );

    store.applyEvent({
      assistantMessageId: "hist-assistant-1",
      assistantMessageEvent: {
        delta: "live answer",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "hist-assistant-1",
      assistantMessageEvent: {
        delta: "live thinking",
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
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "hist-assistant-1",
          message: {
            content: "live answer",
            reasoning_continuation: { fallback_text: "live thinking" },
            role: "assistant",
            tool_calls: [{ function: { name: "edit" }, id: "tool-1" }],
          },
          type: "message",
        },
        {
          id: "hist-tool-1-msg",
          message: {
            content: "updated file",
            role: "tool",
            tool_call_id: "tool-1",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });

    const timeline = store.snapshot().sessionViews.s1.timeline;
    expect(
      timeline.filter(
        (item) => item.type === "message" && item.kind === "assistant" && item.text === "live answer",
      ),
    ).toHaveLength(1);
    expect(
      timeline.filter((item) => item.type === "thinking" && item.text === "live thinking"),
    ).toHaveLength(1);
    expect(
      timeline.filter((item) => item.type === "tool" && item.toolCallId === "tool-1"),
    ).toHaveLength(1);
  });

  it("ignores late deltas after message_end clears the current stream", () => {
    const store = new WebviewStateStore("both");

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
      new Map([["s1", "webview"]]),
      "webview",
    );

    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: {
        delta: "live answer",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: {
        delta: "live thinking",
        kind: "thinking_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      type: "message_end",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: {
        delta: " SHOULD NOT APPEND",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: {
        delta: " SHOULD NOT APPEND",
        kind: "thinking_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });

    const timeline = store.snapshot().sessionViews.s1.timeline;
    const assistant = timeline.find(
      (item) => item.type === "message" && item.kind === "assistant",
    );
    const thinking = timeline.find((item) => item.type === "thinking");
    expect(assistant).toMatchObject({ text: "live answer", type: "message" });
    expect(thinking).toMatchObject({ text: "live thinking", type: "thinking" });
  });

  it("keeps repeated history hydration idempotent after live entries have already converged by id", () => {
    const store = new WebviewStateStore("both");

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
      new Map([["s1", "webview"]]),
      "webview",
    );

    store.applyEvent({
      assistantMessageId: "hist-assistant-1",
      assistantMessageEvent: {
        delta: "live answer",
        kind: "content_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "hist-assistant-1",
      assistantMessageEvent: {
        delta: "live thinking",
        kind: "thinking_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });

    const history = {
      messages: [
        {
          id: "hist-assistant-1",
          message: {
            content: "live answer",
            reasoning_continuation: { fallback_text: "live thinking" },
            role: "assistant",
          },
          type: "message" as const,
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    };

    store.hydrateHistory("s1", history);
    const firstTimeline = store.snapshot().sessionViews.s1.timeline;
    store.hydrateHistory("s1", history);
    const secondTimeline = store.snapshot().sessionViews.s1.timeline;

    expect(secondTimeline).toEqual(firstTimeline);
  });

  it("keeps in-flight assistant tails at the end until disk catches up, then converges in place", () => {
    const store = new WebviewStateStore("both");

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: true, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
      new Map([["s1", "webview"]]),
      "webview",
    );
    store.applySessionState(
      {
        busy: true,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
      "webview",
      "webview",
    );

    const olderHistory = {
      messages: [
        {
          id: "older-user-1",
          message: {
            content: "older prompt",
            role: "user",
          },
          type: "message" as const,
        },
        {
          id: "older-assistant-1",
          message: {
            content: "older answer",
            role: "assistant",
          },
          type: "message" as const,
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    };
    store.hydrateHistory("s1", olderHistory);

    store.applyEvent({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: {
        delta: "live thinking",
        kind: "thinking_delta",
      },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: {
        delta: "live answer",
        kind: "content_delta",
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

    store.hydrateHistory("s1", olderHistory);
    expect(store.snapshot().sessionViews.s1.timeline).toEqual([
      {
        id: "older-user-1",
        kind: "user",
        text: "older prompt",
        type: "message",
      },
      {
        assistantMessageId: "older-assistant-1",
        id: "older-assistant-1",
        kind: "assistant",
        text: "older answer",
        type: "message",
      },
      {
        assistantMessageId: "live-assistant-1",
        id: "live-assistant-1-thinking",
        summaryTitle: null,
        text: "live thinking",
        type: "thinking",
      },
      {
        assistantMessageId: "live-assistant-1",
        id: "live-assistant-1",
        kind: "assistant",
        text: "live answer",
        type: "message",
      },
      {
        args: { path: "src/app.ts" },
        assistantMessageId: "live-assistant-1",
        id: "tool-1",
        isError: false,
        status: "running",
        toolCallId: "tool-1",
        toolName: "edit",
        type: "tool",
      },
    ]);

    store.hydrateHistory("s1", {
      messages: [
        ...olderHistory.messages,
        {
          id: "live-assistant-1",
          message: {
            content: "live answer",
            reasoning_continuation: { fallback_text: "live thinking" },
            role: "assistant",
            tool_calls: [
              {
                function: { arguments: "{\"path\":\"src/app.ts\"}", name: "edit" },
                id: "tool-1",
              },
            ],
          },
          type: "message",
        },
        {
          id: "tool-result-1",
          message: {
            content: "updated file",
            role: "tool",
            tool_call_id: "tool-1",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });

    const finalTimeline = store.snapshot().sessionViews.s1.timeline;
    expect(finalTimeline).toEqual([
      {
        id: "older-user-1",
        kind: "user",
        text: "older prompt",
        type: "message",
      },
      {
        assistantMessageId: "older-assistant-1",
        id: "older-assistant-1",
        kind: "assistant",
        text: "older answer",
        type: "message",
      },
      {
        assistantMessageId: "live-assistant-1",
        id: "live-assistant-1-thinking",
        summaryTitle: null,
        text: "live thinking",
        type: "thinking",
      },
      {
        assistantMessageId: "live-assistant-1",
        id: "live-assistant-1",
        kind: "assistant",
        text: "live answer",
        type: "message",
      },
      {
        args: { path: "src/app.ts" },
        assistantMessageId: "live-assistant-1",
        id: "tool-result-1",
        isError: false,
        status: "complete",
        summary: "updated file",
        toolCallId: "tool-1",
        toolName: "edit",
        type: "tool",
      },
    ]);
    expect(finalTimeline.filter((item) => item.id === "live-assistant-1")).toHaveLength(1);
    expect(
      finalTimeline.filter((item) => item.type === "thinking" && item.id === "live-assistant-1-thinking"),
    ).toHaveLength(1);
    expect(finalTimeline.filter((item) => item.type === "tool" && item.toolCallId === "tool-1")).toHaveLength(1);
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
