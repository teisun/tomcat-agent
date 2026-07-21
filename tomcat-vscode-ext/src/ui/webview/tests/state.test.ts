import { describe, expect, it, vi } from "vitest";

import {
  isWebviewIntent,
  type WebviewMessageBlock,
  type WebviewToolCard,
} from "../protocol";
import {
  buildToolCallToAssistantMap,
  derivePlanActivity,
  WebviewStateStore,
} from "../state";

describe("derivePlanActivity", () => {
  it("derives create_plan counts from args.todos", () => {
    expect(
      derivePlanActivity(
        "create_plan",
        "{\"plan_id\":\"plan-1\",\"path\":\"/workspace/login.plan.md\",\"state\":\"planning\"}",
        {
          goal: "Login refactor plan",
          todos: [
            { content: "Audit transcript rendering", id: "todo-1", status: "completed" },
            { content: "Render update_plan rows", id: "todo-2", status: "pending" },
          ],
        },
      ),
    ).toEqual({
      completed: 1,
      kind: "create",
      stateAfter: "planning",
      title: "Login refactor plan",
      total: 2,
    });
  });

  it("derives checked progress and state transitions for update_plan", () => {
    expect(
      derivePlanActivity(
        "update_plan",
        JSON.stringify({
          applied: 2,
          items: [
            { id: "todo-1", status: "completed" },
            { id: "todo-2", status: "completed" },
            { id: "todo-3", status: "in_progress" },
          ],
          plan_state_after: "executing",
          plan_state_before: "planning",
        }),
        {
          ops: [
            { kind: "set_status", status: "completed", todo_id: "todo-1" },
            { kind: "set_status", status: "completed", todo_id: "todo-2" },
          ],
        },
      ),
    ).toEqual({
      applied: 2,
      checked: 2,
      completed: 2,
      kind: "update",
      stateAfter: "executing",
      stateBefore: "planning",
      total: 3,
    });
  });

  it("keeps non-check edits distinct from fallback and tolerates missing counts", () => {
    expect(
      derivePlanActivity(
        "update_plan",
        JSON.stringify({
          applied: 1,
          items: [
            { id: "todo-1", status: "completed" },
            { id: "todo-2", status: "pending" },
          ],
        }),
        {
          ops: [{ kind: "remove", todo_id: "todo-3" }],
        },
      ),
    ).toEqual({
      applied: 1,
      checked: 0,
      completed: 1,
      kind: "update",
      stateAfter: null,
      stateBefore: null,
      total: 2,
    });

    expect(
      derivePlanActivity(
        "update_plan",
        "{\"applied\":1}",
        {
          ops: [{ kind: "set_status", status: "completed", todo_id: "todo-1" }],
        },
      ),
    ).toEqual({
      applied: 1,
      checked: 1,
      completed: undefined,
      kind: "update",
      stateAfter: null,
      stateBefore: null,
      total: undefined,
    });
  });

  it("returns undefined for malformed or non-plan results", () => {
    expect(derivePlanActivity("update_plan", "not json", { ops: [] })).toBeUndefined();
    expect(derivePlanActivity("read", "{\"ok\":true}", undefined)).toBeUndefined();
  });
});

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

  it("stamps a running create_plan with path metadata when plan.create arrives", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      args: {
        draft: "Keep one create card and many update rows.",
        goal: "Plan tool ux",
        todos: [{ content: "Render the plan card", id: "pt-1", status: "pending" }],
      },
      sessionId: "s1",
      toolCallId: "tc-plan-create",
      toolName: "create_plan",
      type: "tool_execution_start",
    });

    const beforeCreate = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-plan-create");
    expect(beforeCreate?.type === "tool" ? beforeCreate.planPath : undefined).toBeUndefined();
    expect(beforeCreate?.type === "tool" ? beforeCreate.planId : undefined).toBeUndefined();

    store.applyEvent({
      path: "/workspace/plan-tool-ux.plan.md",
      planId: "plan-tool-ux",
      sessionId: "s1",
      state: "planning",
      type: "plan.create",
    });

    const session = store.snapshot().sessionViews.s1;
    const tool = session.timeline.find(
      (item) => item.type === "tool" && item.toolCallId === "tc-plan-create",
    );
    expect(tool?.type === "tool" ? tool.planPath : undefined).toBe(
      "/workspace/plan-tool-ux.plan.md",
    );
    expect(tool?.type === "tool" ? tool.planId : undefined).toBe("plan-tool-ux");
    expect(session.timeline.filter((item) => item.type === "plan")).toEqual([]);
    expect(session.planFile).toMatchObject({
      path: "/workspace/plan-tool-ux.plan.md",
      planId: "plan-tool-ux",
      state: "planning",
    });
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

  it("maps tool.summary_updated onto the matching tool card by toolCallId", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      args: { command: "git status" },
      sessionId: "s1",
      toolCallId: "tc-bash",
      toolName: "bash",
      type: "tool_execution_start",
    });
    store.applyEvent({
      isError: false,
      result: "On branch main",
      sessionId: "s1",
      toolCallId: "tc-bash",
      toolName: "bash",
      type: "tool_execution_end",
    });

    store.applyEvent({
      sessionId: "s1",
      summaryTitle: "Gather git status",
      toolCallId: "tc-bash",
      type: "tool.summary_updated",
    });

    const session = store.snapshot().sessionViews.s1;
    const tool = session.timeline.find(
      (item) => item.type === "tool" && item.toolCallId === "tc-bash",
    );
    expect(tool?.type === "tool" ? tool.summaryTitle : null).toBe("Gather git status");
  });

  it("ignores tool.summary_updated for an unknown toolCallId", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      args: { command: "ls" },
      sessionId: "s1",
      toolCallId: "tc-known",
      toolName: "bash",
      type: "tool_execution_start",
    });

    store.applyEvent({
      sessionId: "s1",
      summaryTitle: "should not attach",
      toolCallId: "tc-missing",
      type: "tool.summary_updated",
    });

    const session = store.snapshot().sessionViews.s1;
    const tool = session.timeline.find(
      (item) => item.type === "tool" && item.toolCallId === "tc-known",
    );
    expect(tool?.type === "tool" ? tool.summaryTitle : undefined).toBeUndefined();
  });

  it("records background bash task metadata on tool_execution_end", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      args: { command: "sleep 12", run_in_background: true },
      sessionId: "s1",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool_execution_start",
    });
    store.applyEvent({
      isError: false,
      result: JSON.stringify({
        logPath: "/tmp/task.log",
        next: "poll task_output",
        startedAtUnixMs: 1_752_000_000_000,
        taskId: "task-123",
      }),
      sessionId: "s1",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool_execution_end",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-bg");
    expect(tool).toMatchObject({
      backgroundRunning: true,
      backgroundTaskId: "task-123",
      status: "complete",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool",
    });
    expect(tool?.type === "tool" ? tool.backgroundExitCode : undefined).toBeUndefined();
  });

  it("flips background bash cards to finished by taskId and ignores unknown task ids", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      args: { command: "sleep 12", run_in_background: true },
      sessionId: "s1",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool_execution_start",
    });
    store.applyEvent({
      isError: false,
      result: JSON.stringify({
        logPath: "/tmp/task.log",
        next: "poll task_output",
        startedAtUnixMs: 1_752_000_000_000,
        taskId: "task-123",
      }),
      sessionId: "s1",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool_execution_end",
    });

    store.applyEvent({
      exitCode: 99,
      sessionId: "s1",
      taskId: "task-missing",
      type: "background_task_finished",
    });
    let tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-bg");
    expect(tool).toMatchObject({
      backgroundRunning: true,
      backgroundTaskId: "task-123",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool",
    });
    expect(tool?.type === "tool" ? tool.backgroundExitCode : undefined).toBeUndefined();

    store.applyEvent({
      exitCode: 23,
      sessionId: "s1",
      taskId: "task-123",
      type: "background_task_finished",
    });

    tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-bg");
    expect(tool).toMatchObject({
      backgroundExitCode: 23,
      backgroundRunning: false,
      backgroundTaskId: "task-123",
      toolCallId: "tc-bg",
      toolName: "bash",
      type: "tool",
    });
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
    );

    store.applyEvent({
      sessionId: "s1",
      title: "Fix transcript UI",
      type: "session.title_updated",
    });

    expect(store.snapshot().sessions[0]?.title).toBe("Fix transcript UI");
  });

  it("replaces a rule title with a later semantic session title", () => {
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
            title: null,
            updatedAt: 1,
          },
        ],
      },
    );

    store.applyEvent({
      sessionId: "s1",
      title: "hello",
      type: "session.title_updated",
    });
    expect(store.snapshot().sessions[0]?.title).toBe("hello");

    store.applyEvent({
      sessionId: "s1",
      title: "Semantic via main model",
      type: "session.title_updated",
    });
    expect(store.snapshot().sessions[0]?.title).toBe("Semantic via main model");
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
    );

    expect(store.snapshot().activeSessionId).toBe("s2");
  });

  it("derives diff stats from file display metadata", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_start",
    });
    store.applyEvent({
      display: {
        added: 3,
        diff: [
          { newLine: 1, oldLine: 1, tag: "ctx", text: "const a = 1;" },
          { newLine: null, oldLine: 2, tag: "del", text: "const b = 2;" },
          { newLine: 2, oldLine: null, tag: "add", text: "const b = 3;" },
        ],
        file: "src/app.ts",
        kind: "file",
        removed: 1,
      },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    const tool = store.snapshot().sessionViews.s1.timeline.find((item) => item.type === "tool");
    expect(tool).toMatchObject({
      diff: [
        { newLine: 1, oldLine: 1, tag: "ctx", text: "const a = 1;" },
        { newLine: null, oldLine: 2, tag: "del", text: "const b = 2;" },
        { newLine: 2, oldLine: null, tag: "add", text: "const b = 3;" },
      ],
      diffStat: {
        added: 3,
        removed: 1,
      },
      toolCallId: "tool-edit-1",
      type: "tool",
    });
  });

  it("leaves diff stats empty when the file display omits counts", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_start",
    });
    store.applyEvent({
      display: { file: "src/app.ts", kind: "file" },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    const tool = store.snapshot().sessionViews.s1.timeline.find((item) => item.type === "tool");
    expect(tool).toMatchObject({
      toolCallId: "tool-edit-1",
      type: "tool",
    });
    expect(tool && "diffStat" in tool ? tool.diffStat : undefined).toBeUndefined();
    expect(tool && "diff" in tool ? tool.diff : undefined).toBeUndefined();
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

  it("hydrates the same update_plan activity from history and live events", () => {
    const historyStore = new WebviewStateStore();
    historyStore.setActiveSession("s1");
    historyStore.hydrateHistory("s1", {
      messages: [
        {
          id: "assistant-1",
          message: {
            role: "assistant",
            tool_calls: [
              {
                function: {
                  arguments: JSON.stringify({
                    ops: [
                      { kind: "set_status", status: "completed", todo_id: "todo-1" },
                      { kind: "set_status", status: "completed", todo_id: "todo-2" },
                    ],
                    path: "/workspace/login.plan.md",
                    plan_id: "plan-1",
                  }),
                  name: "update_plan",
                },
                id: "tc-plan",
              },
            ],
          },
          type: "message",
        },
        {
          id: "tool-plan-result",
          message: {
            content: JSON.stringify({
              applied: 2,
              items: [
                { id: "todo-1", status: "completed" },
                { id: "todo-2", status: "completed" },
                { id: "todo-3", status: "pending" },
              ],
              plan_state_after: "executing",
              plan_state_before: "planning",
            }),
            role: "tool",
            tool_call_id: "tc-plan",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const liveStore = new WebviewStateStore();
    liveStore.setActiveSession("s1");
    liveStore.applyEvent({
      assistantMessageId: "assistant-live",
      assistantMessageEvent: { delta: "planning", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    liveStore.applyEvent({
      args: {
        ops: [
          { kind: "set_status", status: "completed", todo_id: "todo-1" },
          { kind: "set_status", status: "completed", todo_id: "todo-2" },
        ],
        path: "/workspace/login.plan.md",
        plan_id: "plan-1",
      },
      sessionId: "s1",
      toolCallId: "tc-plan",
      toolName: "update_plan",
      type: "tool_execution_start",
    });
    liveStore.applyEvent({
      isError: false,
      result: JSON.stringify({
        applied: 2,
        items: [
          { id: "todo-1", status: "completed" },
          { id: "todo-2", status: "completed" },
          { id: "todo-3", status: "pending" },
        ],
        plan_state_after: "executing",
        plan_state_before: "planning",
      }),
      sessionId: "s1",
      toolCallId: "tc-plan",
      toolName: "update_plan",
      type: "tool_execution_end",
    });

    const historyTool = historyStore
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-plan");
    const liveTool = liveStore
      .snapshot()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tc-plan");

    expect(historyTool?.type === "tool" ? historyTool.planActivity : undefined).toEqual(
      liveTool?.type === "tool" ? liveTool.planActivity : undefined,
    );
    expect(liveTool?.type === "tool" ? liveTool.planPath : undefined).toBe(
      "/workspace/login.plan.md",
    );
    expect(liveTool?.type === "tool" ? liveTool.planId : undefined).toBe("plan-1");
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
    expect(tool?.type === "tool" ? tool.startedAt : undefined).toEqual(expect.any(Number));
  });

  it("stamps startedAt on live tool starts and ignores task_output partialResult countdown state", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(1_752_000_000_000);
    try {
      store.applyEvent({
        args: { block: true, task_id: "task-1", timeout_ms: 600000 },
        sessionId: "s1",
        toolCallId: "tc-task-output",
        toolName: "task_output",
        type: "tool_execution_start",
      });
      store.applyEvent({
        args: { block: true, task_id: "task-1", timeout_ms: 600000 },
        partialResult: {
          phase: "waiting_for_output",
          remainingMs: 123456,
          timeoutMs: 600000,
        },
        sessionId: "s1",
        toolCallId: "tc-task-output",
        toolName: "task_output",
        type: "tool_execution_update",
      } as never);

      const tool = store
        .snapshot()
        .sessionViews.s1.timeline.find(
          (item) => item.type === "tool" && item.toolCallId === "tc-task-output",
        );
      expect(tool).toMatchObject({
        args: { block: true, task_id: "task-1", timeout_ms: 600000 },
        startedAt: 1_752_000_000_000,
        status: "streaming",
        toolCallId: "tc-task-output",
        toolName: "task_output",
        type: "tool",
      });
      expect(tool && "remainingMs" in tool).toBe(false);
      expect(tool && "timeoutMs" in tool).toBe(false);
    } finally {
      nowSpy.mockRestore();
    }
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
    );

    expect(store.snapshot().sessionViews.s1.busy).toBe(false);
  });

  it("keeps busy unchanged when trustBusy is false while still hydrating metadata", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      sessionId: "s1",
      type: "agent_start",
    });

    store.applySessionState(
      {
        busy: false,
        contextRatio: 0.42,
        interrupted: false,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plan-a.plan.md",
        planState: "executing",
        sessionId: "s1",
        thinkingLevel: "high",
      },
      { trustBusy: false },
    );

    const session = store.snapshot().sessionViews.s1;
    expect(session.busy).toBe(true);
    expect(session.contextRatio).toBe(0.42);
    expect(session.model).toBe("gpt-5.4");
    expect(session.planId).toBe("plan-1");
    expect(session.planState).toBe("executing");
  });

  it("treats agent_idle as the only event that returns the session to idle", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applyEvent({
      sessionId: "s1",
      type: "agent_start",
    });
    store.applyEvent({
      error: null,
      messages: [],
      sessionId: "s1",
      type: "agent_end",
    });

    expect(store.snapshot().sessionViews.s1.busy).toBe(true);

    store.applyEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    const snapshot = store.snapshot();
    expect(snapshot.sessionViews.s1.busy).toBe(false);
    expect(snapshot.sessions.find((session) => session.sessionId === "s1")?.busy).toBe(false);
  });

  it("settles stale running tools when agent_idle arrives", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.applyEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-running",
      toolName: "edit",
      type: "tool_execution_start",
    });

    store.applyEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find((item): item is WebviewToolCard => item.type === "tool");
    expect(tool).toMatchObject({
      isError: false,
      status: "complete",
      toolCallId: "tool-running",
      toolName: "edit",
      type: "tool",
    });
  });

  it("preserves summary and error flags when settling stale tools on agent_idle", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    const session = (
      store as unknown as {
        ensureSession(sessionId: string): { timeline: WebviewToolCard[] };
      }
    ).ensureSession("s1");
    session.timeline.push({
      assistantMessageId: "assistant-1",
      id: "tool-streaming",
      isError: true,
      status: "streaming",
      summary: "stale edit rejected",
      toolCallId: "tool-streaming",
      toolName: "edit",
      type: "tool",
    });

    store.applyEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find(
        (item): item is WebviewToolCard => item.type === "tool" && item.toolCallId === "tool-streaming",
      );
    expect(tool).toMatchObject({
      isError: true,
      status: "complete",
      summary: "stale edit rejected",
      toolCallId: "tool-streaming",
      toolName: "edit",
      type: "tool",
    });
  });

  it("does not modify already complete background task cards on agent_idle", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    const session = (
      store as unknown as {
        ensureSession(sessionId: string): { timeline: WebviewToolCard[] };
      }
    ).ensureSession("s1");
    session.timeline.push({
      id: "tool-background",
      isError: false,
      status: "complete",
      summary: "background task queued",
      toolCallId: "tool-background",
      toolName: "bash",
      type: "tool",
    });

    store.applyEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    const tool = store
      .snapshot()
      .sessionViews.s1.timeline.find(
        (item): item is WebviewToolCard => item.type === "tool" && item.toolCallId === "tool-background",
      );
    expect(tool).toMatchObject({
      isError: false,
      status: "complete",
      summary: "background task queued",
      toolCallId: "tool-background",
      toolName: "bash",
      type: "tool",
    });
  });

  it("hydrates plan refs and context ratio from get_state without creating timeline cards", () => {
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
    );

    const session = store.snapshot().sessionViews.s1;
    const planCards = session.timeline.filter((item) => item.type === "plan");
    expect(planCards).toEqual([]);
    expect(session.contextRatio).toBe(0.42);
    expect(session.planFile).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "chat",
    });
  });

  it("keeps live create_plan cards in chronological order when rebuilding against stale history", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    const session = (
      store as unknown as {
        ensureSession(sessionId: string): { timeline: Array<WebviewToolCard | WebviewMessageBlock> };
        ensureRuntime(sessionId: string): { historyEntries: unknown[]; localUserMessageIds: Set<string> };
        rebuildHistoryTimeline(sessionId: string): void;
      }
    ).ensureSession("s1");
    const runtime = (
      store as unknown as {
        ensureRuntime(sessionId: string): { historyEntries: unknown[]; localUserMessageIds: Set<string> };
      }
    ).ensureRuntime("s1");
    session.timeline = [
      { id: "user-1", kind: "user", text: "older prompt", type: "message" },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "older answer",
        type: "message",
      },
      {
        args: {
          goal: "Snake plan",
          path: "/workspace/snake.plan.md",
          todos: [{ content: "Ship the refactor", id: "todo-1", status: "pending" }],
        },
        id: "tool-plan-create",
        isError: false,
        planActivity: {
          completed: 0,
          kind: "create",
          stateAfter: "planning",
          title: "Snake plan",
          total: 1,
        },
        planPath: "/workspace/snake.plan.md",
        planId: "plan-snake",
        status: "complete",
        summary: "{\"plan_id\":\"plan-snake\",\"path\":\"/workspace/snake.plan.md\",\"state\":\"planning\"}",
        toolCallId: "tc-plan-create",
        toolName: "create_plan",
        type: "tool",
      },
      {
        id: "user-2",
        kind: "user",
        text: "latest prompt",
        type: "message",
        deliveryState: "pending",
      },
    ];
    runtime.localUserMessageIds.add("user-2");
    runtime.historyEntries = [
      {
        id: "user-1",
        message: { content: "older prompt", role: "user" },
        type: "message",
      },
    ];

    (
      store as unknown as {
        rebuildHistoryTimeline(sessionId: string): void;
      }
    ).rebuildHistoryTimeline("s1");

    expect(
      store.snapshot().sessionViews.s1.timeline.map((item) => item.id),
    ).toEqual(["user-1", "assistant-1", "tool-plan-create", "user-2"]);
  });

  it("updates plan refs from get_state without moving existing create_plan events", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    const session = (
      store as unknown as {
        ensureSession(sessionId: string): { timeline: Array<WebviewMessageBlock | WebviewToolCard> };
      }
    ).ensureSession("s1");
    session.timeline = [
      { id: "user-1", kind: "user", text: "older prompt", type: "message" },
      {
        args: {
          goal: "Active plan",
          path: "/workspace/active.plan.md",
          todos: [{ content: "First step", id: "todo-1", status: "pending" }],
        },
        id: "tool-plan-create",
        isError: false,
        planActivity: {
          completed: 0,
          kind: "create",
          stateAfter: "planning",
          title: "Active plan",
          total: 1,
        },
        planPath: "/workspace/active.plan.md",
        planId: "plan-1",
        status: "complete",
        summary: "{\"plan_id\":\"plan-1\",\"path\":\"/workspace/active.plan.md\",\"state\":\"planning\"}",
        toolCallId: "tc-plan-create",
        toolName: "create_plan",
        type: "tool",
      },
      { id: "user-2", kind: "user", text: "latest prompt", type: "message" },
    ];

    store.applySessionState(
      {
        busy: false,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/active.plan.md",
        planState: "pending",
        sessionId: "s1",
      },
    );

    const timeline = store.snapshot().sessionViews.s1.timeline;
    expect(timeline.map((item) => item.id)).toEqual([
      "user-1",
      "tool-plan-create",
      "user-2",
    ]);
    expect(store.snapshot().sessionViews.s1.planFile).toMatchObject({
      path: "/workspace/active.plan.md",
      planId: "plan-1",
      state: "pending",
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
  it("replays plan custom entries into ambient plan state without timeline cards", () => {
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
    const notices = session.timeline.filter(
      (item) => item.type === "message" && item.kind === "notice",
    );
    const warnings = session.timeline.filter(
      (item) => item.type === "message" && item.kind === "warn",
    );
    expect(session.timeline.filter((item) => item.type === "plan")).toEqual([]);
    expect(session.planFile).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-1",
      state: "executing",
    });
    expect(session.planState).toBe("executing");
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
    const notice = session.timeline.find(
      (item) => item.type === "message" && item.kind === "notice",
    );
    expect(session.timeline.filter((item) => item.type === "plan")).toEqual([]);
    expect(session.planFile).toMatchObject({
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
  it("stores live plan.todos as ambient state for the active plan", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.applySessionState({
      busy: false,
      model: "gpt-5.4",
      planId: "plan-a",
      planPath: "/workspace/plan-a.plan.md",
      planState: "planning",
      sessionId: "s1",
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
    expect(session.planTodos).toEqual([
      { content: "A step 1", id: "a1", status: "pending" },
      { content: "A step 2", id: "a2", status: "in_progress" },
    ]);
    expect(session.planId).toBe("plan-a");
    expect(session.planFile).toMatchObject({
      path: "/workspace/plan-a.plan.md",
      planId: "plan-a",
      state: "planning",
    });
  });

  it("hydrates plan.todos into ambient state during history replay", () => {
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
    expect(session.planTodos).toEqual([
      { content: "history step", id: "h1", status: "pending" },
    ]);
    expect(session.timeline.filter((item) => item.type === "plan")).toEqual([]);
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

  it("merges refreshed latest history into already-loaded older pages", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      hasMore: true,
      messages: [
        {
          id: "recent-1",
          message: {
            content: "recent prompt",
            role: "user",
          },
          type: "message",
        },
      ],
      nextCursor: "cursor-1",
      sessionId: "s1",
    });
    store.prependHistory("s1", {
      hasMore: false,
      messages: [
        {
          id: "older-1",
          message: {
            content: "older prompt",
            role: "user",
          },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "s1",
    });

    store.hydrateHistory("s1", {
      hasMore: false,
      messages: [
        {
          id: "recent-1",
          message: {
            content: "recent prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "recent-2",
          message: {
            content: "new recent prompt",
            role: "user",
          },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "s1",
    });

    expect(
      store.snapshot().sessionViews.s1.timeline.map((item) =>
        item.type === "message" ? item.id : item.type,
      ),
    ).toEqual(["older-1", "recent-1", "recent-2"]);
  });
});

describe("reference segment hydration", () => {
  it("rehydrates user history with interleaved text and reference segments", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "hist-user-ref",
          message: {
            content: [
              { text: "Inspect ", type: "input_text" },
              {
                label: "app.ts:3-5",
                line_end: 5,
                line_start: 3,
                path: "app.ts",
                ref_kind: "selection",
                text: "const answer = 42;",
                type: "input_reference",
              },
              { text: " please", type: "input_text" },
            ],
            role: "user",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
      upToSeq: null,
    });

    expect(
      store.snapshot().sessionViews.s1.timeline.find(
        (item) => item.type === "message" && item.id === "hist-user-ref",
      ),
    ).toMatchObject({
      id: "hist-user-ref",
      kind: "user",
      segments: [
        { text: "Inspect ", type: "text" },
        {
          kind: "selection",
          label: "app.ts:3-5",
          lineEnd: 5,
          lineStart: 3,
          path: "app.ts",
          text: "const answer = 42;",
          type: "reference",
        },
        { text: " please", type: "text" },
      ],
      text: "Inspect app.ts:3-5 please",
      type: "message",
    });
  });

  it("keeps optimistic user message segments for reference-only prompts", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.appendLocalUserMessage("s1", "app.ts", {
      messageId: "local-ref-only",
      segments: [
        {
          kind: "file",
          label: "app.ts",
          lineEnd: null,
          lineStart: null,
          path: "app.ts",
          text: null,
          type: "reference",
        },
      ],
      submitKind: "prompt",
    });

    expect(
      store.snapshot().sessionViews.s1.timeline.find(
        (item) => item.type === "message" && item.id === "local-ref-only",
      ),
    ).toMatchObject({
      deliveryState: "pending",
      id: "local-ref-only",
      kind: "user",
      segments: [
        {
          kind: "file",
          label: "app.ts",
          lineEnd: null,
          lineStart: null,
          path: "app.ts",
          text: null,
          type: "reference",
        },
      ],
      submitKind: "prompt",
      text: "app.ts",
      type: "message",
    });
  });
});

describe("local user message delivery state", () => {
  it("retains pending and failed user bubbles during rebuild but drops confirmed ones", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
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
    let userMessage = store.snapshot().sessionViews.s1.timeline.find(
      (item) => item.type === "message" && item.id === "user-fixed-id",
    );
    expect(userMessage).toMatchObject({
      deliveryState: "pending",
      id: "user-fixed-id",
      kind: "user",
      text: "draft prompt",
      type: "message",
    });

    store.markLocalUserMessageFailed("s1", "user-fixed-id", "busy", true);
    store.hydrateHistory("s1", baseHistory);
    userMessage = store.snapshot().sessionViews.s1.timeline.find(
      (item) => item.type === "message" && item.id === "user-fixed-id",
    );
    expect(userMessage).toMatchObject({
      deliveryError: "busy",
      deliveryState: "failed",
      id: "user-fixed-id",
      kind: "user",
      retryable: true,
      text: "draft prompt",
      type: "message",
    });

    store.markLocalUserMessageConfirmed("s1", "user-fixed-id");
    userMessage = store.snapshot().sessionViews.s1.timeline.find(
      (item) => item.type === "message" && item.id === "user-fixed-id",
    );
    expect(userMessage).toMatchObject({
      id: "user-fixed-id",
      kind: "user",
      text: "draft prompt",
      type: "message",
    });
    expect(userMessage).not.toHaveProperty("deliveryError");
    expect(userMessage).not.toHaveProperty("deliveryState");
    expect(userMessage).not.toHaveProperty("retryable");

    store.hydrateHistory("s1", baseHistory);
    expect(
      store
        .snapshot()
        .sessionViews.s1.timeline.some(
          (item) => item.type === "message" && item.id === "user-fixed-id",
        ),
    ).toBe(false);
  });

  it("clears local user tracking even when the bubble is already gone", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    const runtime = (
      store as unknown as {
        ensureRuntime(sessionId: string): { localUserMessageIds: Set<string> };
      }
    ).ensureRuntime("s1");
    runtime.localUserMessageIds.add("missing-user-id");

    store.markLocalUserMessageConfirmed("s1", "missing-user-id");

    expect(runtime.localUserMessageIds.has("missing-user-id")).toBe(false);
  });
});

describe("checkpoint history replay", () => {
  it("keeps turn_failed superseded users and their adjacent error entries visible", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
        {
          id: "user-failed",
          message: {
            content: "retry me",
            role: "user",
            superseded: true,
            turn_failed: true,
          },
          type: "message",
        },
        {
          detail: "API 错误 403: <html>forbidden</html>",
          id: "error-1",
          summary: "API 错误 403 · aigateway.sunmi.com · Request-Id req-1",
          type: "error",
        },
        {
          id: "user-2",
          message: {
            content: "visible again",
            role: "user",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    expect(
      session.timeline.map((item) => item.type === "message" ? item.id : item.type),
    ).toEqual(["user-1", "assistant-1", "user-failed", "error-1", "user-2"]);
    const errorBubble = session.timeline.find(
      (item): item is Extract<(typeof session.timeline)[number], { type: "message" }> =>
        item.type === "message" && item.id === "error-1",
    );
    expect(errorBubble?.kind).toBe("error");
    expect(errorBubble?.detailText).toBe("API 错误 403: <html>forbidden</html>");
  });

  it("filters superseded spans and resumes rendering after checkpoint.restore", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
        {
          id: "user-2",
          message: {
            content: "hidden prompt",
            role: "user",
            superseded: true,
          },
          type: "message",
        },
        {
          id: "thinking-hidden",
          text: "hidden reasoning",
          type: "thinking_trace",
        },
        {
          id: "assistant-2",
          message: {
            content: "hidden reply",
            role: "assistant",
            superseded: true,
          },
          type: "message",
        },
        {
          customType: "checkpoint.restore",
          id: "restore-1",
          type: "custom",
        },
        {
          id: "user-3",
          message: {
            content: "visible again",
            role: "user",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const session = store.snapshot().sessionViews.s1;
    expect(
      session.timeline.map((item) => item.type === "message" ? item.id : item.type),
    ).toEqual(["user-1", "assistant-1", "user-3"]);
    expect(
      session.timeline.some(
        (item) => item.type === "thinking" && item.text.includes("hidden reasoning"),
      ),
    ).toBe(false);
  });

  it("closes a superseded span at the next live user message when checkpoint.restore is missing", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
        {
          id: "user-2",
          message: {
            content: "hidden prompt",
            role: "user",
            superseded: true,
          },
          type: "message",
        },
        {
          id: "thinking-hidden",
          text: "hidden reasoning",
          type: "thinking_trace",
        },
        {
          id: "assistant-2",
          message: {
            content: "hidden reply",
            role: "assistant",
            superseded: true,
          },
          type: "message",
        },
        {
          id: "user-3",
          message: {
            content: "visible again",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-3",
          message: {
            content: "new reply",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    expect(
      store.snapshot().sessionViews.s1.timeline.map((item) => item.type === "message" ? item.id : item.type),
    ).toEqual(["user-1", "assistant-1", "user-3", "assistant-3"]);
  });

  it("keeps the latest confirmed user turn visible when checkpoints refresh after turn_end", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
        {
          id: "user-2",
          message: {
            content: "second prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-2",
          message: {
            content: "second reply",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });
    store.appendLocalUserMessage("s1", "latest prompt", {
      messageId: "user-3",
      submitKind: "prompt",
    });
    store.markLocalUserMessageConfirmed("s1", "user-3");
    store.applyEvent({
      assistantMessageEvent: { delta: "latest answer", kind: "content_delta" },
      assistantMessageId: "assistant-3",
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    store.applyEvent({
      assistantMessageId: "assistant-3",
      message: {},
      sessionId: "s1",
      toolCallIds: [],
      toolResults: [],
      turnIndex: 2,
      type: "turn_end",
    });

    const before = store.snapshot().sessionViews.s1.timeline.map((item) => item.id);
    expect(before).toEqual(["user-1", "assistant-1", "user-2", "assistant-2", "user-3", "assistant-3"]);

    store.setCheckpoints("s1", [
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ]);

    const session = store.snapshot().sessionViews.s1;
    expect(session.timeline.map((item) => item.id)).toEqual(before);
    expect(session.timeline.every((item) => item.type !== "checkpoint")).toBe(true);
    expect(session.checkpoints).toEqual([
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        label: null,
        messageAnchor: "assistant-1",
      },
    ]);
  });

  it("keeps checkpoint data separate from timeline items across later rebuilds", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    const history = {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message" as const,
        },
        {
          id: "assistant-1",
          message: {
            content: "",
            role: "assistant",
            tool_calls: [
              {
                function: {
                  arguments: "{}",
                  name: "read_file",
                },
                id: "tool-1",
              },
            ],
          },
          type: "message" as const,
        },
        {
          id: "user-2",
          message: {
            content: "second prompt",
            role: "user",
          },
          type: "message" as const,
        },
      ],
      sessionId: "s1",
    };

    store.hydrateHistory("s1", history);
    store.setCheckpoints("s1", [
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-thinking",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ]);
    store.hydrateHistory("s1", history);

    const session = store.snapshot().sessionViews.s1;
    expect(session.timeline.map((item) => item.id)).toEqual([
      "user-1",
      "assistant-1-thinking",
      "user-2",
    ]);
    expect(session.timeline.every((item) => item.type !== "checkpoint")).toBe(true);
    expect(session.checkpoints).toHaveLength(1);
  });

  it("keeps repeated setCheckpoints calls idempotent", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");

    store.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });

    const checkpoints = [
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ];

    store.setCheckpoints("s1", checkpoints);
    const first = store.snapshot().sessionViews.s1;
    store.setCheckpoints("s1", checkpoints);
    const second = store.snapshot().sessionViews.s1;

    expect(first.timeline).toEqual(second.timeline);
    expect(second.timeline.every((item) => item.type !== "checkpoint")).toBe(true);
    expect(first.checkpoints).toEqual(second.checkpoints);
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
        data: { line: 42, path: "/tmp/file.rs" },
        messageId: "open-1",
        type: "openFile",
      }),
    ).toBe(true);
  });

  it("accepts openDiff intent shape", () => {
    expect(
      isWebviewIntent({
        data: { toolCallId: "tool-1" },
        messageId: "open-diff-1",
        type: "openDiff",
      }),
    ).toBe(true);
  });

  it("accepts retryUserMessage intent shape", () => {
    expect(
      isWebviewIntent({
        data: {
          messageId: "user-1",
          sessionId: "s1",
        },
        messageId: "retry-1",
        type: "retryUserMessage",
      }),
    ).toBe(true);
  });

  it("accepts restoreCheckpoint intent shape", () => {
    expect(
      isWebviewIntent({
        data: {
          checkpointId: "ck-1",
          revertFiles: false,
          sessionId: "s1",
        },
        messageId: "restore-1",
        type: "restoreCheckpoint",
      }),
    ).toBe(true);
  });
});
