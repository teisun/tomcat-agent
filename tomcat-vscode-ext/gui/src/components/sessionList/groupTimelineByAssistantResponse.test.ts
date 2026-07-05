import { describe, expect, it } from "vitest";

import type { WebviewTimelineItem } from "../../types";
import { groupTimelineByAssistantResponse } from "./groupTimelineByAssistantResponse";

function assistantMessage(
  id: string,
  text: string,
  assistantMessageId?: string,
): WebviewTimelineItem {
  return {
    assistantMessageId,
    id,
    kind: "assistant",
    text,
    type: "message",
  };
}

function thinking(id: string, assistantMessageId: string, text = "thinking"): WebviewTimelineItem {
  return {
    assistantMessageId,
    id,
    summaryTitle: null,
    text,
    type: "thinking",
  };
}

function tool(
  id: string,
  toolCallId: string,
  assistantMessageId: string,
): WebviewTimelineItem {
  return {
    assistantMessageId,
    id,
    isError: false,
    status: "complete",
    summary: "result",
    toolCallId,
    toolName: "read",
    type: "tool",
  };
}

describe("groupTimelineByAssistantResponse", () => {
  it("returns empty array for empty timeline", () => {
    expect(groupTimelineByAssistantResponse([])).toEqual([]);
  });

  it("groups preamble thinking and multiple tools for one assistant message", () => {
    const timeline: WebviewTimelineItem[] = [
      thinking("t1", "a1", "inspect files"),
      assistantMessage("a1-msg", "I'll inspect files", "a1"),
      tool("tool-1", "tc1", "a1"),
      tool("tool-2", "tc2", "a1"),
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(1);
    expect(grouped[0]).toMatchObject({
      type: "assistant-response-group",
      assistantMessageId: "a1",
      tools: [{ id: "tool-1" }, { id: "tool-2" }],
    });
  });

  it("keeps text-only assistant收束 as standalone markdown", () => {
    const timeline: WebviewTimelineItem[] = [
      thinking("t1", "a1"),
      tool("tool-1", "tc1", "a1"),
      assistantMessage("closing", "Done summarizing"),
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(2);
    expect(grouped[1]).toMatchObject({ id: "closing", kind: "assistant" });
  });

  it("creates separate groups for consecutive assistant tool rounds", () => {
    const timeline: WebviewTimelineItem[] = [
      thinking("t1", "a1"),
      tool("tool-1", "tc1", "a1"),
      thinking("t2", "a2"),
      tool("tool-2", "tc2", "a2"),
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(2);
    expect(grouped[0]).toMatchObject({ assistantMessageId: "a1" });
    expect(grouped[1]).toMatchObject({ assistantMessageId: "a2" });
  });

  it("leaves unassigned tools standalone", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "orphan",
        isError: false,
        status: "complete",
        summary: "x",
        toolCallId: "tc-orphan",
        toolName: "bash",
        type: "tool",
      },
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(1);
    expect(grouped[0]).toMatchObject({ id: "orphan", type: "tool" });
  });

  it("does not group user plan or approval items", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "hello",
        type: "message",
      },
      {
        id: "plan-1",
        path: "/tmp/plan.plan.md",
        planId: "p1",
        state: "planning",
        type: "plan",
      },
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(2);
  });

  it("groups read/bash/web_search from one turn into a single group", () => {
    const timeline: WebviewTimelineItem[] = [
      thinking("t1", "a1", "deciding"),
      assistantMessage("a1-msg", "I'll run a few tools", "a1"),
      tool("tool-read", "tc-read", "a1"),
      tool("tool-bash", "tc-bash", "a1"),
      tool("tool-web", "tc-web", "a1"),
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(1);
    expect(grouped[0]).toMatchObject({
      type: "assistant-response-group",
      assistantMessageId: "a1",
    });
    const group = grouped[0] as { tools: { id: string }[] };
    expect(group.tools.map((entry) => entry.id)).toEqual([
      "tool-read",
      "tool-bash",
      "tool-web",
    ]);
  });

  it("keeps one assistantMessageId grouped even when the preamble arrives after tools", () => {
    const timeline: WebviewTimelineItem[] = [
      tool("tool-1", "tc-1", "a1"),
      thinking("t1", "a1", "late thinking"),
      assistantMessage("a1-msg", "Late preamble", "a1"),
    ];

    const grouped = groupTimelineByAssistantResponse(timeline);
    expect(grouped).toHaveLength(1);
    expect(grouped[0]).toMatchObject({
      assistantMessageId: "a1",
      preamble: { id: "a1-msg" },
      thinking: { id: "t1" },
      tools: [{ id: "tool-1" }],
      type: "assistant-response-group",
    });
  });
});
