import { describe, expect, it } from "vitest";

import { isWebviewIntent, type WebviewPlanFileCard } from "../protocol";
import {
  buildToolCallToAssistantMap,
  WebviewStateStore,
} from "../state";

describe("WebviewStateStore wire routing", () => {
  it("upserts plan.todos and session.todos", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      sessionId: "s1",
      todos: [{ content: "Plan step", id: "p1", status: "pending" }],
      type: "plan.todos",
    });
    store.applyEvent({
      sessionId: "s1",
      todos: [{ content: "Chat step", id: "s1", status: "in_progress" }],
      type: "session.todos",
    });

    const session = store.snapshot().sessionViews.s1;
    expect(session.planTodos).toEqual([
      { content: "Plan step", id: "p1", status: "pending" },
    ]);
    expect(session.sessionTodos).toEqual([
      { content: "Chat step", id: "s1", status: "in_progress" },
    ]);
  });

  it("maps turn_end summaryTitle onto the matching tool group", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      assistantMessageId: "server-assistant-1",
      assistantMessageEvent: { delta: "thinking", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      args: { path: "/tmp/a.rs" },
      sessionId: "s1",
      toolCallId: "tc-1",
      toolName: "read",
      type: "tool_execution_start",
    });

    store.applyEvent({
      assistantMessageId: "server-assistant-1",
      sessionId: "s1",
      summaryTitle: "Reviewed 2 files",
      toolCallIds: ["tc-1"],
      toolResults: [{}],
      turnIndex: 0,
      message: {},
      type: "turn_end",
    });

    const thinking = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "thinking");
    expect(thinking?.type === "thinking" ? thinking.summaryTitle : null).toBe(
      "Reviewed 2 files",
    );
  });

  it("maps turn.summary_updated onto a live tool-only group", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: { delta: "thinking", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      args: { command: "git status" },
      sessionId: "s1",
      toolCallId: "tc-live",
      toolName: "bash",
      type: "tool_execution_start",
    });

    store.applyEvent({
      sessionId: "s1",
      summaryTitle: "Reviewed repository status",
      toolCallIds: ["tc-live"],
      turnIndex: 1,
      type: "turn.summary_updated",
    });

    const session = store.snapshot().sessionViews.s1;
    const thinking = session.timeline.find((item) => item.type === "thinking");
    const tool = session.timeline.find((item) => item.type === "tool");
    expect(thinking?.type === "thinking" ? thinking.summaryTitle : null).toBe(
      "Reviewed repository status",
    );
    expect(thinking?.type === "thinking" ? thinking.assistantMessageId : undefined).toBe(
      tool?.type === "tool" ? tool.assistantMessageId : undefined,
    );
  });

  it("updates session tab title on session.title_updated", () => {
    const store = new WebviewStateStore();
    store.syncSessionList(
      {
        activeSessionId: "s1",
        scope: "live",
        sessions: [
          {
            busy: false,
            isCurrent: true,
            sessionId: "s1",
            title: "Placeholder title",
            updatedAt: 1,
          },
        ],
      },
      new Map(),
      "webview",
    );

    store.applyEvent({
      sessionId: "s1",
      title: "Fix transcript UI",
      type: "session.title_updated",
    });

    expect(store.snapshot().sessions[0]?.title).toBe("Fix transcript UI");
  });

  it("does not override an existing active session during session list refresh", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.syncSessionList(
      {
        activeSessionId: "s2",
        scope: "disk",
        sessions: [
          {
            busy: false,
            isCurrent: false,
            sessionId: "s1",
            title: "Session A",
            updatedAt: 1,
          },
          {
            busy: true,
            isCurrent: true,
            sessionId: "s2",
            title: "Session B",
            updatedAt: 2,
          },
        ],
      },
      new Map(),
      "webview",
    );

    expect(store.snapshot().activeSessionId).toBe("s1");
  });

  it("adopts the server active session when the webview has none yet", () => {
    const store = new WebviewStateStore();

    store.syncSessionList(
      {
        activeSessionId: "s2",
        scope: "disk",
        sessions: [
          {
            busy: true,
            isCurrent: true,
            sessionId: "s2",
            title: "Session B",
            updatedAt: 2,
          },
        ],
      },
      new Map(),
      "webview",
    );

    expect(store.snapshot().activeSessionId).toBe("s2");
  });
});

describe("history tool attribution", () => {
  it("buildToolCallToAssistantMap maps tool call ids to assistant message id", () => {
    const map = buildToolCallToAssistantMap([
      {
        id: "assistant-1",
        message: {
          role: "assistant",
          tool_calls: [{ function: { name: "read" }, id: "tc-1" }],
        },
        type: "message",
      },
    ]);

    expect(map.get("tc-1")).toBe("assistant-1");
  });

  it("parseHistoryEntry backfills assistantMessageId and args on tool cards", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.hydrateHistory("s1", {
      messages: [
        {
          id: "assistant-1",
          message: {
            role: "assistant",
            thinking_text: "inspect",
            tool_calls: [
              {
                function: { arguments: "{\"command\":\"ls\"}", name: "bash" },
                id: "tc-1",
              },
            ],
          },
          type: "message",
        },
        {
          id: "tool-result-1",
          message: {
            content: "output",
            role: "tool",
            tool_call_id: "tc-1",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool");
    expect(tool?.type === "tool" ? tool.assistantMessageId : undefined).toBe("assistant-1");
    expect(tool?.type === "tool" ? tool.args : undefined).toEqual({ command: "ls" });
  });

  it("hydrates persisted summary_title even when assistant had no thinking_text", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.hydrateHistory("s1", {
      messages: [
        {
          id: "assistant-1",
          message: {
            role: "assistant",
            summary_title: "Reviewed 2 files",
            tool_calls: [
              {
                function: { arguments: "{\"path\":\"/tmp/a.rs\"}", name: "read" },
                id: "tc-1",
              },
            ],
          },
          type: "message",
        },
        {
          id: "tool-result-1",
          message: {
            content: "file content",
            role: "tool",
            tool_call_id: "tc-1",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const thinking = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "thinking");
    expect(thinking?.type === "thinking" ? thinking.summaryTitle : null).toBe(
      "Reviewed 2 files",
    );
    expect(thinking?.type === "thinking" ? thinking.text : undefined).toBe("");
  });

  it("assigns assistantMessageId for history thinking-only turns", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.hydrateHistory("s1", {
      messages: [
        {
          id: "assistant-thinking-1",
          message: {
            content: "Done.",
            thinking_text: "Inspect the existing plan mode history flow.",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    const thinking = session.timeline.find((item) => item.type === "thinking");
    const assistant = session.timeline.find(
      (item) => item.type === "message" && item.kind === "assistant",
    );
    expect(thinking?.type === "thinking" ? thinking.assistantMessageId : undefined).toBe(
      "assistant-thinking-1",
    );
    expect(assistant?.type === "message" ? assistant.assistantMessageId : undefined).toBe(
      "assistant-thinking-1",
    );
  });

  it("live tool_execution_start writes activeAssistantId and args", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: { delta: "working", kind: "content_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      args: { command: "cargo test" },
      sessionId: "s1",
      toolCallId: "tc-live",
      toolName: "bash",
      type: "tool_execution_start",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool");
    expect(tool?.type === "tool" ? tool.args : undefined).toEqual({ command: "cargo test" });
    expect(tool?.type === "tool" ? tool.assistantMessageId : undefined).toBeTruthy();
  });

  it("live multi-tool turn shares one assistantMessageId", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: { delta: "working", kind: "content_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      args: { path: "a.rs" },
      sessionId: "s1",
      toolCallId: "tc-read",
      toolName: "read",
      type: "tool_execution_start",
    });
    store.applyEvent({
      args: { command: "git status" },
      sessionId: "s1",
      toolCallId: "tc-bash",
      toolName: "bash",
      type: "tool_execution_start",
    });

    const tools = store
      .snapshot()
      .sessionViews.s1.timeline.filter((item) => item.type === "tool");
    const readTool = tools.find((t) => t.type === "tool" && t.toolCallId === "tc-read");
    const bashTool = tools.find((t) => t.type === "tool" && t.toolCallId === "tc-bash");
    expect(readTool?.type === "tool" ? readTool.assistantMessageId : undefined).toBeTruthy();
    expect(bashTool?.type === "tool" ? bashTool.assistantMessageId : undefined).toBe(
      readTool?.type === "tool" ? readTool.assistantMessageId : undefined,
    );
  });

  it("converges idle timelines exactly onto persisted disk entries without leftovers", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: { delta: "trace", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      assistantMessageEvent: { delta: "final answer", kind: "content_delta" },
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
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "edit",
      type: "tool_execution_end",
    });
    store.applyEvent({
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      type: "message_end",
    });

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "assistant-1",
          message: {
            content: "final answer",
            reasoning_continuation: { fallback_text: "trace" },
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
    });

    expect(store.snapshot().sessionViews.s1.timeline).toEqual([
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1-thinking",
        summaryTitle: null,
        text: "trace",
        type: "thinking",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "final answer",
        type: "message",
      },
      {
        args: { path: "src/app.ts" },
        assistantMessageId: "assistant-1",
        id: "tool-result-1",
        isError: false,
        status: "complete",
        summary: "updated file",
        toolCallId: "tool-1",
        toolName: "edit",
        type: "tool",
      },
    ]);
  });
});

describe("session state hydration", () => {
  it("treats interrupted get_state payloads as idle for UI purposes", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applySessionState(
      {
        busy: true,
        interrupted: true,
        model: "gpt-5.4",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    expect(store.snapshot().sessionViews.s1.busy).toBe(false);
  });

  it("hydrates plan cards and context ratio from get_state without duplicating cards", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applySessionState(
      {
        busy: false,
        contextRatio: 0.42,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plan-a.plan.md",
        planState: "planning",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    store.applyEvent({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      sessionId: "s1",
      state: "executing",
      type: "plan.build",
    });
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plan-a.plan.md",
        planState: "pending",
        sessionId: "s1",
      },
      null,
      "webview",
    );
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    const session = store.snapshot().sessionViews.s1;
    const planCards = session.timeline.filter(
      (item) => item.type === "plan" && item.path === "/workspace/plan-a.plan.md",
    );
    expect(planCards).toHaveLength(1);
    expect(session.contextRatio).toBe(0.42);
    expect(session.planFile).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "chat",
    });
    expect(planCards[0]).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "chat",
    });
  });

  it("marks running tools interrupted and deduplicates the warn card", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "edit",
      type: "tool_execution_start",
    });

    store.applyEvent({
      partialTextLen: 0,
      sessionId: "s1",
      toolResultsCount: 0,
      type: "agent_interrupted",
    });
    store.applyEvent({
      partialTextLen: 0,
      sessionId: "s1",
      toolResultsCount: 0,
      type: "agent_interrupted",
    });

    const session = store.snapshot().sessionViews.s1;
    const tool = session.timeline.find((item) => item.type === "tool");
    const warnings = session.timeline.filter(
      (item) => item.type === "message" && item.kind === "warn",
    );
    expect(tool).toMatchObject({
      status: "interrupted",
      summary: "Interrupted",
    });
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatchObject({ text: "Tomcat turn interrupted" });
  });
});

describe("custom history replay", () => {
  it("replays plan custom entries into one card and preserves current state", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plan-a.plan.md",
        planState: "executing",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    store.hydrateHistory("s1", {
      messages: [
        {
          event: "plan.enter",
          id: "enter-1",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.create",
          id: "create-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-1",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.update",
          id: "update-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-1",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.pending",
          id: "pending-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-1",
          state: "pending",
          type: "custom",
        },
        {
          event: "plan.complete",
          id: "complete-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-1",
          state: "completed",
          type: "custom",
        },
        {
          event: "plan.review",
          id: "review-1",
          plan_id: "plan-1",
          summary: "looks good",
          type: "custom",
        },
        {
          event: "plan.verify",
          id: "verify-1",
          plan_id: "plan-1",
          type: "custom",
          verdict: "pass",
        },
        {
          event: "plan.review.warning",
          id: "warn-1",
          plan_id: "plan-1",
          reason: "rounds_exhausted",
          type: "custom",
        },
        {
          event: "plan.exit",
          id: "exit-1",
          state: "chat",
          type: "custom",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    const planCards = session.timeline.filter(
      (item) => item.type === "plan" && item.path === "/workspace/plan-a.plan.md",
    );
    const notices = session.timeline.filter(
      (item) => item.type === "message" && item.kind === "notice",
    );
    const warnings = session.timeline.filter(
      (item) => item.type === "message" && item.kind === "warn",
    );
    expect(planCards).toHaveLength(1);
    expect(planCards[0]).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "executing",
    });
    expect(notices).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ text: "Tomcat plan review: looks good" }),
        expect.objectContaining({ text: "Tomcat plan verify: pass" }),
      ]),
    );
    expect(warnings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ text: "Tomcat plan warning: rounds_exhausted" }),
      ]),
    );
  });

  it("drops leading orphan tool entries until the assistant head arrives", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      hasMore: true,
      messages: [
        {
          id: "tool-result-1",
          message: {
            content: "output",
            role: "tool",
            tool_call_id: "tc-1",
          },
          type: "message",
        },
      ],
      nextCursor: "cursor-1",
      sessionId: "s1",
    });

    expect(store.snapshot().sessionViews.s1.timeline).toEqual([]);

    store.prependOlderHistory("s1", {
      hasMore: false,
      messages: [
        {
          id: "assistant-1",
          message: {
            role: "assistant",
            tool_calls: [
              {
                function: { arguments: "{\"command\":\"ls\"}", name: "bash" },
                id: "tc-1",
              },
            ],
          },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "s1",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool");
    expect(tool?.type).toBe("tool");
    expect(tool?.type === "tool" ? tool.assistantMessageId : undefined).toBe("assistant-1");
  });

  it("keeps current plan state authoritative after prepending older plan history", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plan-a.plan.md",
        planState: "executing",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    store.hydrateHistory("s1", {
      messages: [],
      sessionId: "s1",
    });
    store.prependOlderHistory("s1", {
      hasMore: false,
      messages: [
        {
          event: "plan.create",
          id: "create-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-1",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.review",
          id: "review-1",
          plan_id: "plan-1",
          summary: "looks good",
          type: "custom",
        },
      ],
      nextCursor: null,
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    const planCard = session.timeline.find(
      (item) => item.type === "plan" && item.path === "/workspace/plan-a.plan.md",
    );
    const notice = session.timeline.find(
      (item) => item.type === "message" && item.kind === "notice",
    );
    expect(planCard).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "executing",
    });
    expect(notice).toMatchObject({
      kind: "notice",
      text: "Tomcat plan review: looks good",
      type: "message",
    });
  });

  it("renders only boundary branch summaries and hides preheat summaries", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "summary-preheat",
          isBoundary: false,
          summary: "preheat",
          type: "branch_summary",
        },
        {
          coveredCount: 8,
          id: "summary-boundary",
          isBoundary: true,
          summary: "Earlier turns were summarized.",
          type: "branch_summary",
        },
      ],
      sessionId: "s1",
    });

    const boundaries = store
      .snapshot()
      .sessionViews.s1.timeline.filter((item) => item.type === "boundary");
    expect(boundaries).toEqual([
      {
        coveredCount: 8,
        id: "summary-boundary",
        summary: "Earlier turns were summarized.",
        type: "boundary",
      },
    ]);
  });
});

describe("plan.todos routing", () => {
  it("routes live plan.todos onto the matching card by planId without cross-talk", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-a",
      sessionId: "s1",
      state: "planning",
      type: "plan.create",
    });
    store.applyEvent({
      path: "/workspace/plan-b.plan.md",
      planId: "plan-b",
      sessionId: "s1",
      state: "planning",
      type: "plan.create",
    });
    store.applyEvent({
      planId: "plan-a",
      sessionId: "s1",
      todos: [
        { content: "A step 1", id: "a1", status: "pending" },
        { content: "A step 2", id: "a2", status: "in_progress" },
      ],
      type: "plan.todos",
    });

    const session = store.snapshot().sessionViews.s1;
    const findCard = (planId: string) =>
      session.timeline.find(
        (item): item is WebviewPlanFileCard =>
          item.type === "plan" && item.planId === planId,
      );
    expect(findCard("plan-a")?.todos).toEqual([
      { content: "A step 1", id: "a1", status: "pending" },
      { content: "A step 2", id: "a2", status: "in_progress" },
    ]);
    expect(findCard("plan-b")?.todos).toBeUndefined();
    expect(session.planTodos).toHaveLength(2);
  });

  it("attaches plan.todos to the card during history replay", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-a",
        planPath: "/workspace/plan-a.plan.md",
        planState: "planning",
        sessionId: "s1",
      },
      null,
      "webview",
    );

    store.hydrateHistory("s1", {
      messages: [
        {
          event: "plan.create",
          id: "create-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-a",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.todos",
          id: "todos-1",
          plan_id: "plan-a",
          todos: [
            { content: "history step", id: "h1", status: "pending" },
          ],
          type: "custom",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    const card = session.timeline.find(
      (item): item is WebviewPlanFileCard =>
        item.type === "plan" && item.planId === "plan-a",
    );
    expect(card?.todos).toEqual([
      { content: "history step", id: "h1", status: "pending" },
    ]);
    expect(session.planTodos).toEqual([
      { content: "history step", id: "h1", status: "pending" },
    ]);
  });

  it("does not overwrite existing live planTodos when history has no todos for the active plan", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-a",
        planPath: "/workspace/plan-a.plan.md",
        planState: "planning",
        sessionId: "s1",
      },
      null,
      "webview",
    );
    store.applyEvent({
      planId: "plan-a",
      sessionId: "s1",
      todos: [{ content: "live step", id: "l1", status: "pending" }],
      type: "plan.todos",
    });

    store.hydrateHistory("s1", {
      messages: [
        {
          event: "plan.create",
          id: "create-1",
          path: "/workspace/plan-a.plan.md",
          plan_id: "plan-a",
          state: "planning",
          type: "custom",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    expect(session.planTodos).toEqual([
      { content: "live step", id: "l1", status: "pending" },
    ]);
  });
});

describe("openFile intent protocol", () => {
  it("accepts loadOlderHistory intent shape", () => {
    expect(
      isWebviewIntent({
        data: {
          sessionId: "s1",
        },
        messageId: "load-older-1",
        type: "loadOlderHistory",
      }),
    ).toBe(true);
  });

  it("accepts openFile intent shape", () => {
    expect(
      isWebviewIntent({
        data: { path: "/tmp/file.rs" },
        messageId: "open-1",
        type: "openFile",
      }),
    ).toBe(true);
  });
});
