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
  it("reveals flat read file details inside the existing group fold", () => {
    const onOpenFile = vi.fn();
    const { container } = render(<ThinkingGroup group={buildGroup({ thinking: undefined, tools: [{
      type: "tool", id: "read-batch", toolCallId: "batch-call", toolName: "read", status: "complete", isError: false,
      summary: "RAW_READ_BODY_SENTINEL", display: { kind: "files", summary: "", files: [{ file: "/workspace/guide.md", range: "L1-9 (9 lines)" }] },
    }] })} onOpenFile={onOpenFile} />);
    expect(screen.queryByTestId("tool-row")).toBeNull();
    fireEvent.click(screen.getByTestId("thinking-group-toggle"));
    expect(screen.queryByTestId("disclosure-card")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-file-range").textContent).toBe("L1-9 (9 lines)");
    expect(container.textContent).not.toContain("RAW_READ_BODY_SENTINEL");
    fireEvent.click(screen.getByTestId("file-chip"));
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/guide.md");
  });

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

  it("keeps a long single-file title on the group header while the live ticker stays separate", () => {
    const fileName = "plan_cli_vs_code_ci_release_guards_cargo_npm_c0ee6d86.plan.md";
    const filePath = `/workspace/${fileName}`;
    const titleText = `Read file ${fileName}`;
    const { container } = render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: null,
            text: "Inspect workspace",
            type: "thinking",
          },
          tools: [
            {
              args: { path: filePath },
              assistantMessageId: "assistant-1",
              display: { file: filePath, kind: "file" },
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

    const title = screen.getByTestId("thinking-group-title");
    const ticker = screen.getByTestId("group-activity-ticker");
    const toggle = screen.getByTestId("thinking-group-toggle");
    expect(title.textContent).toBe(titleText);
    expect(title.getAttribute("title")).toBe(titleText);
    expect(title.className).toContain("tc-thinking-box__title");
    expect(ticker.textContent).toContain(titleText);
    expect(toggle.nextElementSibling).toBe(ticker);
    expect(container.querySelector(".tc-group-ticker__line")).toBeTruthy();
    expect(container.querySelector(".tc-group-ticker__icon")).toBeTruthy();

    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
    expect(screen.getByTestId("thinking-group-title").textContent).toBe(titleText);
    expect(screen.getAllByTestId("tool-row")).toHaveLength(1);
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

  it("keeps a long summary title verbatim with the single-line group title contract", () => {
    const titleText = "Used 4 tools for finding coffee shops across Shenzhen and nearby districts";
    render(
      <ThinkingGroup
        group={buildGroup({
          thinking: {
            assistantMessageId: "assistant-1",
            id: "think-1",
            summaryTitle: titleText,
            text: "Mixed batch of reads and edits.",
            type: "thinking",
          },
        })}
        onOpenFile={vi.fn()}
      />,
    );

    const title = screen.getByTestId("thinking-group-title");
    expect(title.textContent).toBe(titleText);
    expect(title.getAttribute("title")).toBe(titleText);
    expect(title.className).toContain("tc-thinking-box__title");
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
