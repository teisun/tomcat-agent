import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ThinkingGroup } from "./ThinkingGroup";
import type { AssistantResponseGroup } from "./sessionList/groupTimelineByAssistantResponse";

function buildGroup(overrides: Partial<AssistantResponseGroup> = {}): AssistantResponseGroup {
  return {
    assistantMessageId: "assistant-1",
    preamble: {
      assistantMessageId: "assistant-1",
      id: "msg-1",
      kind: "assistant",
      text: "I'll review files.",
      type: "message",
    },
    thinking: {
      assistantMessageId: "assistant-1",
      id: "think-1",
      summaryTitle: "Reviewed 3 files",
      text: "Need to inspect sources",
      type: "thinking",
    },
    tools: [
      {
        assistantMessageId: "assistant-1",
        id: "tool-1",
        isError: false,
        status: "complete",
        summary: "a",
        toolCallId: "tc-1",
        toolName: "read",
        type: "tool",
      },
      {
        assistantMessageId: "assistant-1",
        id: "tool-2",
        isError: false,
        status: "complete",
        summary: "b",
        toolCallId: "tc-2",
        toolName: "read",
        type: "tool",
      },
    ],
    type: "assistant-response-group",
    ...overrides,
  };
}

describe("ThinkingGroup", () => {
  it("shows summaryTitle in header and keeps preamble above fold header", () => {
    render(
      <ThinkingGroup
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Reviewed 3 files");
    expect(screen.getByText("I'll review files.")).toBeTruthy();
  });

  it("does not render tool rows while folded", () => {
    render(
      <ThinkingGroup
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryAllByTestId("tool-row")).toHaveLength(0);
  });

  it("renders thinking and tool rows when expanded", () => {
    render(
      <ThinkingGroup
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    expect(screen.getByTestId("thinking-group-body").textContent).toContain("Need to inspect");
    expect(screen.getAllByTestId("tool-row")).toHaveLength(2);
  });

  it("applies shimmer when streaming without summaryTitle", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: null,
            text: "Still thinking",
            type: "thinking",
          },
          tools: [
            {
              assistantMessageId: "assistant-1",
              id: "tool-1",
              isError: false,
              status: "streaming",
              summary: "partial",
              toolCallId: "tc-1",
              toolName: "bash",
              type: "tool",
            },
          ],
        })}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).toContain(
      "tc-thinking__title--shimmer",
    );
  });

  it("falls back to a clean tool-derived title when summaryTitle is missing", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: null,
            text: "Inspect workspace",
            type: "thinking",
          },
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Reviewed 2 files");
  });

  it("replaces raw tool-argument summary titles with a clean tool label", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: 'ask_question {"questions":[{"id":"style"}]}',
            text: "Need the user to choose a direction.",
            type: "thinking",
          },
          tools: [
            {
              assistantMessageId: "assistant-1",
              id: "tool-1",
              isError: false,
              status: "complete",
              summary: '{"answers":[],"cancelled":true}',
              toolCallId: "tc-1",
              toolName: "ask_question",
              type: "tool",
            },
          ],
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Asked question");
  });

  it("shows a loading status icon while streaming and a search icon for context groups when done", () => {
    const { rerender } = render(
      <ThinkingGroup
        group={buildGroup()}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-loading",
    );

    rerender(
      <ThinkingGroup
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-search",
    );
  });

  it("applies shimmer while streaming with no tools yet", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: null,
            text: "Still thinking",
            type: "thinking",
          },
          tools: [],
        })}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).toContain(
      "tc-thinking__title--shimmer",
    );
  });

  it("ignores summaryTitle when the group only contains thinking text", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: "Ran wc -l README.md",
            text: "Still reasoning about the command output.",
            type: "thinking",
          },
          tools: [],
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Thinking");
  });

  it("keeps the grouped plan header but hides the inner non-error plan tool row", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: "Creating plan",
            text: "Let me structure the work first.",
            type: "thinking",
          },
          tools: [
            {
              assistantMessageId: "assistant-1",
              id: "tool-plan",
              isError: false,
              status: "streaming",
              summary: "Creating plan",
              toolCallId: "tc-plan",
              toolName: "create_plan",
              type: "tool",
            },
          ],
        })}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Creating plan");
    expect(screen.getByTestId("thinking-group-body").textContent).toContain("structure the work");
    expect(screen.queryByTestId("tool-row")).toBeNull();
  });

  it("still renders failed plan tool rows for debugging feedback", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          tools: [
            {
              assistantMessageId: "assistant-1",
              id: "tool-plan-error",
              isError: true,
              status: "complete",
              summary: "Unable to update plan",
              toolCallId: "tc-plan-error",
              toolName: "update_plan",
              type: "tool",
            },
          ],
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    expect(screen.getByTestId("tool-row").textContent).toContain("Updated plan");
  });
});
