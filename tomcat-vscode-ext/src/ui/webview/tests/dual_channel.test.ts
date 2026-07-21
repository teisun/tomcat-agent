import { describe, expect, it } from "vitest";

import { WebviewStateStore } from "../state";

describe("webview dual-channel state store", () => {
  it("maps state snapshots, history, and live events into timeline state", () => {
    const store = new WebviewStateStore();

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
    );
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
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
          request: expect.objectContaining({ requestId: "ask-1" }),
          type: "approval",
        }),
      ]),
    );
    expect(historicThinkingIndex).toBeGreaterThanOrEqual(0);
    expect(historicAssistantIndex).toBeGreaterThan(historicThinkingIndex);
    expect(liveThinkingIndex).toBeGreaterThanOrEqual(0);
    expect(liveAssistantIndex).toBeGreaterThan(liveThinkingIndex);
    expect(snapshot.sessionViews.s1.timeline.some((item) => item.type === "plan")).toBe(false);
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
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
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
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
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
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
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

  it("drops stale persisted user items that are not tracked as in-flight during rebuild", () => {
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: true, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
    );
    store.applySessionState(
      {
        busy: true,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
    );

    store.appendMessage("s1", "user", "ghost prompt", {
      preferredId: "ghost-user-1",
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

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "recent-user-1",
          message: {
            content: "recent prompt",
            role: "user",
          },
          type: "message" as const,
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });

    expect(store.snapshot().sessionViews.s1.timeline).toEqual([
      {
        id: "recent-user-1",
        kind: "user",
        segments: [{ text: "recent prompt", type: "text" }],
        text: "recent prompt",
        type: "message",
      },
      {
        assistantMessageId: "live-assistant-1",
        id: "live-assistant-1",
        kind: "assistant",
        text: "live answer",
        type: "message",
      },
    ]);
  });

  it("keeps in-flight user bubbles until history catches up, then converges them by id", () => {
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
    );
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
    );

    const baseHistory = {
      messages: [
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
    store.hydrateHistory("s1", baseHistory);
    store.appendLocalUserMessage("s1", "draft prompt", {
      messageId: "user-fixed-id",
      submitKind: "prompt",
    });

    store.hydrateHistory("s1", baseHistory);
    let timeline = store.snapshot().sessionViews.s1.timeline;
    expect(timeline.filter((item) => item.type === "message" && item.id === "user-fixed-id")).toHaveLength(1);
    expect(
      timeline.find((item) => item.type === "message" && item.id === "user-fixed-id"),
    ).toMatchObject({
      deliveryState: "pending",
      id: "user-fixed-id",
      kind: "user",
      text: "draft prompt",
      type: "message",
    });

    store.hydrateHistory("s1", {
      messages: [
        ...baseHistory.messages,
        {
          id: "user-fixed-id",
          message: {
            content: "draft prompt",
            role: "user",
          },
          type: "message" as const,
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });

    timeline = store.snapshot().sessionViews.s1.timeline;
    const userMessages = timeline.filter(
      (item): item is Extract<(typeof timeline)[number], { type: "message" }> =>
        item.type === "message" && item.id === "user-fixed-id",
    );
    expect(userMessages).toHaveLength(1);
    expect(userMessages[0]).toEqual({
      id: "user-fixed-id",
      kind: "user",
      segments: [{ text: "draft prompt", type: "text" }],
      text: "draft prompt",
      type: "message",
    });
  });

  it("drops confirmed user bubbles outside the latest window but restores them when older history loads", () => {
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: false, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
    );
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
    );

    const latestWindowMessages = Array.from({ length: 80 }, (_, index) => ({
      id: `recent-user-${index + 1}`,
      message: {
        content: `recent prompt ${index + 1}`,
        role: "user",
      },
      type: "message" as const,
    }));

    store.hydrateHistory("s1", {
      hasMore: true,
      messages: latestWindowMessages,
      nextCursor: "older-cursor",
      sessionId: "s1",
      upToSeq: null,
    });

    store.appendLocalUserMessage("s1", "older confirmed prompt", {
      messageId: "older-confirmed-user",
      submitKind: "prompt",
    });
    store.markLocalUserMessageConfirmed("s1", "older-confirmed-user");

    store.appendLocalUserMessage("s1", "latest confirmed prompt", {
      messageId: "latest-confirmed-user",
      submitKind: "prompt",
    });
    store.markLocalUserMessageConfirmed("s1", "latest-confirmed-user");

    store.hydrateHistory("s1", {
      hasMore: true,
      messages: [
        ...latestWindowMessages.slice(1),
        {
          id: "latest-confirmed-user",
          message: {
            content: "latest confirmed prompt",
            role: "user",
          },
          type: "message" as const,
        },
      ],
      nextCursor: "older-cursor",
      sessionId: "s1",
      upToSeq: null,
    });

    let timeline = store.snapshot().sessionViews.s1.timeline;
    expect(
      timeline.some((item) => item.type === "message" && item.id === "older-confirmed-user"),
    ).toBe(false);
    const latestUserMessages = timeline.filter(
      (item): item is Extract<(typeof timeline)[number], { type: "message" }> =>
        item.type === "message" && item.id === "latest-confirmed-user",
    );
    expect(latestUserMessages).toHaveLength(1);
    expect(latestUserMessages[0]).toEqual({
      id: "latest-confirmed-user",
      kind: "user",
      segments: [{ text: "latest confirmed prompt", type: "text" }],
      text: "latest confirmed prompt",
      type: "message",
    });

    store.prependHistory("s1", {
      hasMore: false,
      messages: [
        {
          id: "older-confirmed-user",
          message: {
            content: "older confirmed prompt",
            role: "user",
          },
          type: "message" as const,
        },
      ],
      nextCursor: null,
      sessionId: "s1",
      upToSeq: null,
    });

    timeline = store.snapshot().sessionViews.s1.timeline;
    const olderUserMessages = timeline.filter(
      (item): item is Extract<(typeof timeline)[number], { type: "message" }> =>
        item.type === "message" && item.id === "older-confirmed-user",
    );
    expect(olderUserMessages).toHaveLength(1);
    expect(olderUserMessages[0]).toEqual({
      id: "older-confirmed-user",
      kind: "user",
      segments: [{ text: "older confirmed prompt", type: "text" }],
      text: "older confirmed prompt",
      type: "message",
    });
  });

  it("keeps in-flight assistant tails at the end until disk catches up, then converges in place", () => {
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "disk",
        sessions: [{ busy: true, isCurrent: true, sessionId: "s1", title: null, updatedAt: 123 }],
      },
    );
    store.applySessionState(
      {
        busy: true,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
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
        segments: [{ text: "older prompt", type: "text" }],
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
        startedAt: expect.any(Number),
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
        segments: [{ text: "older prompt", type: "text" }],
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
    const store = new WebviewStateStore();

    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: "s1",
      },
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
