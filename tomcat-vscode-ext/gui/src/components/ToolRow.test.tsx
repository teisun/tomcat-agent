import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewToolCard } from "../types";
import { ToolRow } from "./ToolRow";

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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={onOpenFile}
      />,
    );

    fireEvent.click(screen.getByTestId("file-chip"));
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/README.md");
  });

  it("bash row shows Ran command and expands output without terminal button", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "cargo test" },
          status: "complete",
          summary: "test output",
          toolName: "bash",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Ran");
    expect(screen.getByTestId("tool-row-cmd").textContent).toBe("cargo test");
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-result").textContent).toBe("test output");
    expect(screen.queryByRole("button", { name: /terminal/i })).toBeNull();
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
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain('Searched "rust async"');
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByText("Rust async book")).toBeTruthy();
  });

  it("complete rows default folded and streaming rows default expanded", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({ status: "complete" })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("tool-row-body")).toBeNull();

    rerender(
      <ToolRow
        item={buildTool({ status: "streaming" })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-body")).toBeTruthy();
  });

  it("does not render inline check icon in row label", () => {
    render(
      <ToolRow
        item={buildTool()}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(document.querySelector(".tc-tool-row .codicon-check")).toBeNull();
  });

  it("bash row renders the command inside a code element", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "git status --short" },
          status: "complete",
          summary: "M file",
          toolName: "bash",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    const cmd = screen.getByTestId("tool-row-cmd");
    expect(cmd.tagName).toBe("CODE");
    expect(cmd.textContent).toBe("git status --short");
  });

  it("grep row appends result count from summary", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { pattern: "foo" },
          status: "complete",
          summary: "Found 2 results\nfile.rs:10:foo\nfile.rs:20:foo",
          toolName: "grep",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    const label = screen.getByTestId("tool-row-label").textContent ?? "";
    expect(label).toContain("Searched foo");
    expect(label).toContain("2 results");
  });

  it("search_workspace row shows workspace search label", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { query: "config" },
          status: "complete",
          summary: "hit",
          toolName: "search_workspace",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Searched workspace for config",
    );
  });

  it("edit row uses the edit codicon", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/a.rs" },
          display: { file: "/workspace/a.rs", kind: "file" },
          toolName: "edit",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(document.querySelector(".tc-thinking-tool-wrapper .codicon-edit")).toBeTruthy();
  });

  it("maps additional built-in tools to readable labels and distinct icons", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { name: "sdk" },
          summary: "Loaded skill",
          toolName: "load_skill",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Loaded skill sdk");
    expect(document.querySelector(".tc-thinking-tool-wrapper .codicon-book")).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/src" },
          summary: "src\nREADME.md",
          toolName: "list_dir",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Listed /workspace/src");
    expect(document.querySelector(".tc-thinking-tool-wrapper .codicon-folder")).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { key: "log.level", value: "debug" },
          summary: "Updated log.level",
          toolName: "config_set",
        })}
        onApplyEdit={vi.fn()}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Updated config log.level");
    expect(document.querySelector(".tc-thinking-tool-wrapper .codicon-gear")).toBeTruthy();
  });
});
