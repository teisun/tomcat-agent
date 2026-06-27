import { describe, expect, it } from "vitest";

import { routeWireEventType } from "../../../serveClient/sessionRouter";
import { isWebviewIntent } from "../protocol";
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

  it("maps turn_end summaryTitle onto thinking block", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      assistantMessageEvent: { delta: "thinking", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });

    store.applyEvent({
      sessionId: "s1",
      summaryTitle: "Reviewed 2 files",
      toolResults: [],
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

  it("live tool_execution_start writes activeAssistantId and args", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
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
});

describe("routeWireEventType", () => {
  it("recognizes new wire events", () => {
    expect(routeWireEventType("plan.todos")).toBe("plan_todos");
    expect(routeWireEventType("session.todos")).toBe("session_todos");
    expect(routeWireEventType("session.title_updated")).toBe("session_title");
    expect(routeWireEventType("turn_end")).toBe("turn_end");
  });
});

describe("openFile intent protocol", () => {
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
