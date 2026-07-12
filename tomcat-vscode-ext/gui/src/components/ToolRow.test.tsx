import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewToolCard } from "../types";
import { toolCategory, ToolRow } from "./ToolRow";

function buildTool(overrides: Partial<WebviewToolCard> = {}): WebviewToolCard {
  return {
    id: "tool-1",
    isError: false,
    status: "complete",
    summary: "file contents here",
    toolCallId: "tc-1",
    toolName: "read",
    type: "tool",
    ...overrides,
  };
}

describe("ToolRow", () => {
  it("read row renders FileChip and opens file on click", () => {
    const onOpenFile = vi.fn();
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/README.md" },
          display: { file: "/workspace/README.md", kind: "file" },
        })}
        onOpenFile={onOpenFile}
      />,
    );

    fireEvent.click(screen.getByTestId("file-chip"));
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/README.md");
  });

  it("edit row shows diff badges and routes the View diff action", () => {
    const onOpenDiff = vi.fn();
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/a.rs" },
          diff: [
            { newLine: 1, oldLine: 1, tag: "ctx", text: "fn main() {" },
            { newLine: null, oldLine: 2, tag: "del", text: "  old();" },
            { newLine: 2, oldLine: null, tag: "add", text: "  new();" },
          ],
          diffStat: { added: 4, removed: 2 },
          display: { file: "/workspace/a.rs", kind: "file" },
          status: "complete",
          toolName: "edit",
        })}
        onOpenDiff={onOpenDiff}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Edited");
    expect(screen.getByTestId("tool-row-diff-added").textContent).toBe("+4");
    expect(screen.getByTestId("tool-row-diff-removed").textContent).toBe("-2");
    expect(screen.getByTestId("tool-row-open-diff")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tool-row-open-diff"));
    expect(onOpenDiff).toHaveBeenCalledWith("tc-1");
    expect(screen.queryByRole("button", { name: /apply/i })).toBeNull();
  });

  it("bash row uses a terminal block and stays collapsed when complete", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "cargo test" },
          status: "complete",
          summary: "test output",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Ran");
    expect(screen.getByTestId("tool-row-cmd").textContent).toBe("cargo test");
    expect(screen.queryByTestId("tool-row-terminal")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("test output");
  });

  it("bash row auto expands when it errors", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "cargo test" },
          isError: true,
          status: "complete",
          summary: "command failed",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("command failed");
  });

  it("web_search row expands hits list", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { query: "rust async" },
          status: "complete",
          summary: "Rust async book\nTokio tutorial",
          toolName: "web_search",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain('Searched "rust async"');
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByText("Rust async book")).toBeTruthy();
  });

  it("ask_question renders an always-visible answer card", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            questions: [
              {
                id: "style",
                options: [{ id: "run-gun", label: "Run-and-gun", recommended: true }],
                prompt: "Which style?",
              },
            ],
          },
          summary: JSON.stringify({
            answers: [
              {
                optionIds: ["run-gun"],
                pickedRecommended: true,
                questionId: "style",
              },
            ],
            cancelled: false,
          }),
          toolName: "ask_question",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("tool-row-toggle")).toBeNull();
    expect(screen.getByTestId("answer-card").textContent).toContain("Answers");
    expect(screen.getByTestId("answer-option-style").textContent).toContain("Run-and-gun");
  });

  it("toolCategory maps built-ins into the new buckets", () => {
    expect(toolCategory("edit")).toBe("edit");
    expect(toolCategory("bash")).toBe("command");
    expect(toolCategory("ask_question")).toBe("answer");
    expect(toolCategory("read")).toBe("context");
    expect(toolCategory("create_plan")).toBe("other");
    expect(toolCategory("unknown_tool")).toBe("other");
  });

  it("context rows keep the minimalist style and stay collapsed by default", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { query: "config" },
          status: "complete",
          summary: "hit",
          toolName: "search_workspace",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row").getAttribute("data-tool-category")).toBe("context");
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Searched workspace for config",
    );
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
  });

  it("maps additional built-in tools to readable labels and distinct icons", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { name: "sdk" },
          summary: "Loaded skill",
          toolName: "load_skill",
        })}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Loaded skill sdk");
    expect(document.querySelector(".codicon-book")).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/readme.md" },
          display: { file: "/workspace/readme.md", kind: "file" },
          summary: "# readme",
          toolName: "read",
        })}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Read");
    expect(document.querySelector(".codicon-eye")).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/src" },
          summary: "src\nREADME.md",
          toolName: "list_dir",
        })}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Listed /workspace/src");
    expect(document.querySelector(".codicon-folder")).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { key: "log.level", value: "debug" },
          summary: "Updated log.level",
          toolName: "config_set",
        })}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Updated config log.level");
    expect(document.querySelector(".codicon-gear")).toBeTruthy();
  });

  it("keeps running tools with no content collapsed and hides the toggle", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/new-file.ts" },
          status: "streaming",
          summary: undefined,
          toolName: "write",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("tool-row-toggle")).toBeNull();
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
    expect(screen.getByTestId("tool-row-running-indicator").textContent).toBe("...");
  });

  it("accepts snake_case ask_question results from the transcript", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            questions: [
              {
                id: "deploy_target",
                options: [{ id: "staging", label: "Staging" }],
                prompt: "Deploy where?",
              },
            ],
          },
          summary: JSON.stringify({
            answers: [
              {
                option_ids: ["staging"],
                picked_recommended: false,
                question_id: "deploy_target",
              },
            ],
            cancelled: false,
          }),
          toolName: "ask_question",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("answer-card-question").textContent).toContain("Deploy where?");
    expect(screen.getByTestId("answer-option-deploy_target").textContent).toContain("Staging");
  });

  it("stops showing the running indicator for interrupted tools", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/a.rs" },
          status: "interrupted",
          summary: "[interrupted]",
          toolName: "edit",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("tool-row-running-indicator")).toBeNull();
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Interrupted edit");
    expect(screen.getByTestId("tool-row-body").textContent).toContain("Interrupted");
  });
});
