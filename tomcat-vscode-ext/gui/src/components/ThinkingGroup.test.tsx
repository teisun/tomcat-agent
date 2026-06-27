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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryAllByTestId("tool-row")).toHaveLength(0);
  });

  it("renders thinking and tool rows when expanded", () => {
    render(
      <ThinkingGroup
        group={buildGroup()}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).toContain(
      "tc-thinking__title--shimmer",
    );
  });

  it("falls back to Tomcat · Thinking when summaryTitle missing and not streaming", () => {
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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toContain("Tomcat · Thinking");
  });

  it("shows a loading status icon while streaming and a check when done", () => {
    const { rerender } = render(
      <ThinkingGroup
        group={buildGroup()}
        isStreaming
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-loading",
    );

    rerender(
      <ThinkingGroup
        group={buildGroup()}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-check",
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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).toContain(
      "tc-thinking__title--shimmer",
    );
  });
});
