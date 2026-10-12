import { describe, expect, it } from "vitest";

import type { AssistantResponseGroup } from "./sessionList/groupTimelineByAssistantResponse";
import { partitionAssistantResponseGroup } from "./TranscriptView";

function buildGroup(
  overrides: Partial<AssistantResponseGroup> = {},
): AssistantResponseGroup {
  return {
    assistantMessageId: "assistant-1",
    preamble: undefined,
    thinking: undefined,
    tools: [],
    type: "assistant-response-group",
    ...overrides,
  };
}

function tool(
  id: string,
  toolName: string,
  args?: Record<string, unknown>,
  overrides: Partial<AssistantResponseGroup["tools"][number]> = {},
): AssistantResponseGroup["tools"][number] {
  return {
    args,
    assistantMessageId: "assistant-1",
    id,
    isError: false,
    status: "complete",
    summary: `${toolName} result`,
    toolCallId: `tc-${id}`,
    toolName,
    type: "tool",
    ...overrides,
  };
}

describe("partitionAssistantResponseGroup", () => {
  it("keeps pure context turns inside a single collapsed segment", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        tools: [tool("read-1", "read"), tool("search-1", "search_workspace")],
      }),
    );

    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      type: "context-group",
      group: {
        tools: [{ id: "read-1" }, { id: "search-1" }],
      },
    });
  });

  it("keeps pure action turns fully visible without a context segment", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        tools: [tool("edit-1", "edit"), tool("bash-1", "bash")],
      }),
    );

    expect(entries.map((entry) => entry.type)).toEqual([
      "action-tool",
      "action-tool",
    ]);
  });

  it("flushes alternating context and action tools in time order", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        tools: [
          tool("read-1", "read"),
          tool("edit-1", "edit"),
          tool("search-1", "search_workspace"),
          tool("bash-1", "bash"),
        ],
      }),
    );

    expect(entries.map((entry) => entry.type)).toEqual([
      "context-group",
      "action-tool",
      "context-group",
      "action-tool",
    ]);
  });

  it("keeps a single edit visible even when thinking exists", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        thinking: {
          assistantMessageId: "assistant-1",
          id: "thinking-1",
          summaryTitle: null,
          text: "Need to patch the file.",
          type: "thinking",
        },
        tools: [tool("edit-1", "edit")],
      }),
    );

    expect(entries.map((entry) => entry.type)).toEqual([
      "context-group",
      "action-tool",
    ]);
  });

  it("promotes a plan workflow tool to an action segment so ToolRow can own the UX", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        tools: [
          {
            assistantMessageId: "assistant-1",
            id: "plan-1",
            isError: false,
            status: "streaming",
            summary: "Creating plan",
            toolCallId: "tc-plan-1",
            toolName: "create_plan",
            type: "tool",
          },
        ],
      }),
    );

    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      type: "action-tool",
      tool: { id: "plan-1", toolName: "create_plan" },
    });
  });

  it("promotes blocking task_output rows but keeps non-blocking task tools grouped", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        tools: [
          tool(
            "task-blocking",
            "task_output",
            {
              block: true,
              task_id: "task-1",
              wait_ms: 10_000,
            },
            { status: "running" },
          ),
          tool("task-read", "task_output", {
            block: false,
            task_id: "task-1",
            wait_ms: 0,
          }),
          tool("task-stop", "task_stop", { task_id: "task-1" }),
          tool("task-list", "task_list"),
        ],
      }),
    );

    expect(entries).toHaveLength(2);
    expect(entries[0]).toMatchObject({
      type: "action-tool",
      tool: { id: "task-blocking", toolName: "task_output" },
    });
    expect(entries[1]).toMatchObject({
      type: "context-group",
      group: {
        tools: [
          { id: "task-read", toolName: "task_output" },
          { id: "task-stop", toolName: "task_stop" },
          { id: "task-list", toolName: "task_list" },
        ],
      },
    });
  });

  it("creates a thinking-only context segment when no tools exist", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        thinking: {
          assistantMessageId: "assistant-1",
          id: "thinking-1",
          summaryTitle: null,
          text: "Just thinking.",
          type: "thinking",
        },
      }),
    );

    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      type: "context-group",
      group: {
        thinking: { id: "thinking-1" },
        tools: [],
      },
    });
  });

  it("does not create an empty thinking box for action-only summary shells", () => {
    const entries = partitionAssistantResponseGroup(
      buildGroup({
        thinking: {
          assistantMessageId: "assistant-1",
          id: "thinking-1",
          summaryTitle: "Edited file",
          text: "",
          type: "thinking",
        },
        tools: [tool("edit-1", "edit")],
      }),
    );

    expect(entries.map((entry) => entry.type)).toEqual(["action-tool"]);
  });
});
