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
    expect(screen.getByTestId("thinking-group-title").className).not.toContain(
      "tc-thinking__title--shimmer",
    );
    expect(screen.getByText("I'll review files.")).toBeTruthy();
  });

  it("does not render tool rows while folded", () => {
    render(
      <ThinkingGroup
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
    expect(screen.queryAllByTestId("tool-row")).toHaveLength(0);
  });

  it("shows a collapsed activity ticker for live groups with tools", () => {
    const { container } = render(
      <ThinkingGroup
        group={buildGroup({
          tools: [
            {
              args: { path: "/workspace/demo.ts" },
              assistantMessageId: "assistant-1",
              display: { file: "/workspace/demo.ts", kind: "file" },
              id: "tool-1",
              isError: false,
              status: "complete",
              summary: "done",
              toolCallId: "tc-1",
              toolName: "read",
              type: "tool",
            },
          ],
        })}
        isLive
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("group-activity-ticker")).toBeTruthy();
    expect(screen.getByText("Read file demo.ts")).toBeTruthy();
    expect(container.querySelector(".tc-group-ticker__line")).toBeTruthy();
    expect(container.querySelector(".tc-group-ticker__icon")).toBeTruthy();
  });

  it("renders thinking and tool rows when expanded", () => {
    const { container } = render(
      <ThinkingGroup
        isLive
        group={buildGroup()}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
    expect(screen.getByTestId("thinking-group-body").tagName).toBe("PRE");
    expect(screen.getByTestId("thinking-group-body").textContent).toContain("Need to inspect");
    expect(screen.getAllByTestId("tool-row")).toHaveLength(2);
    expect(container.querySelector(".tc-thinking-tool-wrapper")).toBeTruthy();
    expect(container.querySelector(".tc-thinking-icon")).toBeTruthy();
  });

  it("reflows adjacent bold-only thinking headings inside expanded groups without enabling markdown", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: "Reviewed 3 files",
            text: "**Identifying local code modifications** **Comparing exports and test feasibility** **Planning non-bash UI testing approach**",
            type: "thinking",
          },
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    const body = screen.getByTestId("thinking-group-body");
    expect(body.tagName).toBe("PRE");
    expect(body.textContent).toContain(
      [
        "**Identifying local code modifications**",
        "**Comparing exports and test feasibility**",
        "**Planning non-bash UI testing approach**",
      ].join("\n"),
    );
    expect(body.querySelector("strong")).toBeNull();
  });

  it("stays collapsed and applies shimmer when streaming without summaryTitle", () => {
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
    expect(screen.getByTestId("thinking-group-toggle").getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryAllByTestId("tool-row")).toHaveLength(0);
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

  it("renders a 'Used N tools for <purpose>' summary title verbatim (treated as clean)", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: "Used 4 tools for finding coffee shops in Shenzhen",
            text: "Mixed batch of reads and edits.",
            type: "thinking",
          },
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").textContent).toBe(
      "Used 4 tools for finding coffee shops in Shenzhen",
    );
  });

  it("applies shimmer to a clean summary title only while the group is streaming", () => {
    const group = buildGroup({
      thinking: {
        assistantMessageId: "assistant-1",
        id: "think-1",
        summaryTitle: "Used 4 tools for finding coffee shops in Shenzhen",
        text: "Mixed batch of reads and edits.",
        type: "thinking",
      },
    });
    const { rerender } = render(
      <ThinkingGroup
        group={group}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).toContain(
      "tc-thinking__title--shimmer",
    );

    rerender(
      <ThinkingGroup
        group={group}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-title").className).not.toContain(
      "tc-thinking__title--shimmer",
    );
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

  it("keeps a static search icon for tool groups, even while streaming", () => {
    const { rerender } = render(
      <ThinkingGroup
        group={buildGroup()}
        isStreaming
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-search",
    );
    expect(screen.getByTestId("thinking-group-status").className).not.toContain(
      "codicon-loading",
    );
    expect(screen.getByTestId("thinking-group-status").className).not.toContain(
      "tc-codicon-spin",
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
    expect(screen.getByTestId("thinking-group-status").className).toContain(
      "codicon-lightbulb",
    );
  });

  it("shows summaryTitle when the group only contains thinking text", () => {
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

    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Ran wc -l README.md");
  });

  it("does not apply any extra suppression when a plan tool is explicitly grouped", () => {
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: "Updated plan for transcript cleanup",
            text: "Let me structure the work first.",
            type: "thinking",
          },
          tools: [
            {
              assistantMessageId: "assistant-1",
              id: "tool-plan",
              isError: false,
              planActivity: {
                checked: 1,
                completed: 2,
                kind: "update",
                total: 4,
              },
              planId: "plan-1",
              planPath: "/tmp/demo.plan.md",
              status: "complete",
              summary: "{\"applied\":1}",
              toolCallId: "tc-plan",
              toolName: "update_plan",
              type: "tool",
            },
          ],
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    expect(screen.getByTestId("thinking-group-title").textContent).toBe(
      "Updated plan for transcript cleanup",
    );
    expect(screen.getByTestId("thinking-group-body").textContent).toContain("structure the work");
    expect(screen.getByTestId("tool-row").textContent).toContain("Checked 1 · 2/4");
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
    expect(screen.getByTestId("tool-row").textContent).toContain("update_plan failed");
    expect(screen.getByTestId("tool-row-body").textContent).toContain("Unable to update plan");
  });
});
