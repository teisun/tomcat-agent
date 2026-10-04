import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WebviewToolCard } from "../types";
import {
  buildFlatLabel,
  clampTaskOutputBudget,
  commandBinaries,
  formatCountdown,
  isActionTool,
  toolCategory,
  ToolRow,
} from "./ToolRow";

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
  afterEach(() => {
    vi.useRealTimers();
  });

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
    const { container } = render(
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Edited",
    );
    expect(screen.getByTestId("tool-row-diff-added").textContent).toBe("+4");
    expect(screen.getByTestId("tool-row-diff-removed").textContent).toBe("-2");
    expect(screen.getByTestId("tool-row-open-diff")).toBeTruthy();
    expect(screen.getByTestId("tool-row-open-diff").textContent).toContain(
      "View diff",
    );
    expect(screen.getByTestId("tool-row-open-diff").className).not.toContain(
      "tc-tool-row__action-link--plan",
    );
    expect(
      screen
        .getByTestId("tool-row-open-diff")
        .querySelector(".tc-tool-row__action-link-chevron"),
    ).toBeNull();
    expect(container.querySelector(".tc-tool-row__leading-icon")).toBeNull();
    expect(screen.getByTestId("disclosure-card-leading-icon")).toBeTruthy();
    expect(
      screen.getByTestId("diff-view-preview").closest(".tc-disclosure-card"),
    ).toBeTruthy();
    fireEvent.click(screen.getByTestId("tool-row-open-diff"));
    expect(onOpenDiff).toHaveBeenCalledWith("tc-1");
    expect(screen.queryByRole("button", { name: /apply/i })).toBeNull();
  });

  it.each(["edit", "write", "hashline_edit"])("opens the saved diff when %s contains an omitted gap", (toolName) => {
    const onOpenDiff = vi.fn();
    const onOpenFile = vi.fn();
    render(
      <ToolRow
        item={buildTool({
          diff: [
            { newLine: 1, oldLine: 1, tag: "ctx", text: "before gap" },
            { newLine: null, oldLine: null, tag: "gap", text: "90 unmodified lines" },
            { newLine: 92, oldLine: null, tag: "add", text: "after gap" },
          ],
          diffStat: { added: 1, removed: 0 },
          display: { file: "/workspace/a.rs", kind: "file" },
          toolName,
        })}
        onOpenDiff={onOpenDiff}
        onOpenFile={onOpenFile}
      />,
    );

    expect(screen.queryByTestId("tool-row-open-file")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-open-diff"));
    expect(onOpenDiff).toHaveBeenCalledWith("tc-1");
    expect(onOpenFile).not.toHaveBeenCalled();
  });

  it("offers the current file when the inline diff was truncated", () => {
    const onOpenFile = vi.fn();
    render(
      <ToolRow
        item={buildTool({
          diff: [{ newLine: 1, oldLine: null, tag: "add", text: "kept prefix" }],
          diffStat: { added: 2_001, removed: 0 },
          diffTruncated: true,
          display: { file: "/workspace/a.rs", kind: "file" },
          toolName: "edit",
        })}
        onOpenDiff={vi.fn()}
        onOpenFile={onOpenFile}
      />,
    );

    expect(screen.getByTestId("diff-view-truncated").textContent).toContain(
      "Diff 过大已截断，无法查看本次对比",
    );
    expect(screen.getByRole("button", { name: "打开当前文件" })).toBeTruthy();
    expect(screen.queryByTestId("tool-row-open-diff")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-open-file"));
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/a.rs");
  });

  it("shows an expired diff summary without an expansion action", () => {
    render(
      <ToolRow
        item={buildTool({
          diffExpired: true,
          diffStat: { added: 109, removed: 0 },
          display: {
            added: 109,
            diff: null,
            expired: true,
            file: "/workspace/old.rs",
            kind: "file",
            removed: 0,
          },
          status: "complete",
          toolName: "edit",
        })}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("diff-view-expired").textContent).toContain(
      "超过 7 天保留期",
    );
    expect(screen.queryByTestId("tool-row-open-diff")).toBeNull();
  });

  it.each([
    { diffExpired: true, diff: [{ tag: "add" as const, text: "expired content" }] },
    { diff: [{ tag: "ctx" as const, text: "unchanged context" }] },
  ])("does not offer View diff for expired or unchanged saved content", (overrides) => {
    render(
      <ToolRow
        item={buildTool({
          ...overrides,
          display: { file: "/workspace/a.rs", kind: "file" },
          toolName: "edit",
        })}
        onOpenDiff={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("tool-row-open-diff")).toBeNull();
  });

  it("edit row preview stays anchored to the first real change", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/a.rs" },
          diff: [
            { newLine: 1, oldLine: 1, tag: "ctx", text: "line 1" },
            { newLine: 2, oldLine: 2, tag: "ctx", text: "line 2" },
            { newLine: 3, oldLine: 3, tag: "ctx", text: "line 3" },
            { newLine: 4, oldLine: 4, tag: "ctx", text: "line 4" },
            { newLine: 5, oldLine: 5, tag: "ctx", text: "line 5" },
            { newLine: 6, oldLine: 6, tag: "ctx", text: "line 6" },
            { newLine: 7, oldLine: 7, tag: "ctx", text: "line 7" },
            { newLine: 8, oldLine: 8, tag: "ctx", text: "line 8" },
            { newLine: 9, oldLine: 9, tag: "ctx", text: "line 9" },
            { newLine: 10, oldLine: 10, tag: "ctx", text: "line 10" },
            { newLine: null, oldLine: 11, tag: "del", text: "line 11 old" },
            { newLine: 11, oldLine: null, tag: "add", text: "line 11 new" },
            { newLine: 12, oldLine: 12, tag: "ctx", text: "line 12" },
            { newLine: 13, oldLine: 13, tag: "ctx", text: "line 13" },
            { newLine: 14, oldLine: 14, tag: "ctx", text: "line 14" },
            { newLine: 15, oldLine: 15, tag: "ctx", text: "line 15" },
            { newLine: 16, oldLine: 16, tag: "ctx", text: "line 16" },
            { newLine: 17, oldLine: 17, tag: "ctx", text: "line 17" },
            { newLine: 18, oldLine: 18, tag: "ctx", text: "line 18" },
          ],
          display: { file: "/workspace/a.rs", kind: "file" },
          toolName: "edit",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    const preview = screen.getByTestId("diff-view-preview").textContent ?? "";
    expect(preview).toContain("line 10");
    expect(preview).toContain("line 11 old");
    expect(preview).toContain("line 11 new");
    expect(preview).not.toContain("line 18");
  });

  it("bash row uses a terminal block and stays collapsed when complete", () => {
    const { container } = render(
      <ToolRow
        item={buildTool({
          args: { command: "cargo test" },
          status: "complete",
          summary: "line 1\nline 2\nline 3\nline 4\nline 5\nline 6",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Ran");
    // The full command moved to the terminal body; the header keeps a short command tag.
    expect(screen.queryByTestId("tool-row-cmd")).toBeNull();
    expect(screen.getByTestId("tool-row-cmd-tags").textContent).toBe("cargo");
    expect(container.querySelector(".tc-tool-row__leading-icon")).toBeNull();
    expect(screen.getByTestId("disclosure-card-leading-icon")).toBeTruthy();
    expect(screen.queryByTestId("tool-row-terminal")).toBeNull();
    const preview =
      screen.getByTestId("terminal-output-preview").textContent ?? "";
    expect(preview).toContain("$ cargo test");
    expect(preview).not.toContain("line 1");
    expect(preview).toContain("line 2");
    expect(preview).toContain("line 6");
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "$ cargo test",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "line 1",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "line 6",
    );
  });

  it("bash row rebuilds a complete command from command plus argv", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            args: [
              "test",
              "--lib",
              "--manifest-path",
              "tomcat/Cargo.toml",
              "system_prompt_reflects_runtime_permission_skill_and_plugin_tool_changes",
            ],
            command: "cargo",
          },
          status: "complete",
          summary: "test result: ok",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-tags").textContent).toBe("cargo");
    const expected =
      "$ cargo test --lib --manifest-path tomcat/Cargo.toml system_prompt_reflects_runtime_permission_skill_and_plugin_tool_changes";
    expect(screen.getByTestId("terminal-output-preview").textContent).toContain(
      expected,
    );
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      expected,
    );
  });

  it("bash row quotes argv values without losing argument boundaries", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            args: ["plain", "hello world", 'say "hi"', ""],
            command: "printf",
          },
          status: "complete",
          summary: "done",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("terminal-output-preview").textContent).toContain(
      `$ printf plain 'hello world' 'say "hi"' ''`,
    );
  });

  it("bash header shows the utility purpose title and command-name tags", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "git status && echo '---' && git log -1" },
          status: "complete",
          summary: "On branch main\n---\ncommit abc",
          summaryTitle: "Gather git status and recent commit",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe(
      "Gather git status and recent commit",
    );
    // Deduped command-name tags parsed client-side from the full command.
    expect(screen.getByTestId("tool-row-cmd-tags").textContent).toBe(
      "git, echo",
    );
    // Full command surfaces as a `$ …` prompt line in the terminal body.
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "$ git status && echo '---' && git log -1",
    );
  });

  it("bash header falls back to a placeholder verb before the summary title arrives", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { command: "npm run build" },
          status: "complete",
          summary: "built ok",
          summaryTitle: null,
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe("Ran");
    expect(screen.getByTestId("tool-row-cmd-tags").textContent).toBe("npm");
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

    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "command failed",
    );
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      'Searched "rust async"',
    );
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByText("Rust async book")).toBeTruthy();
  });

  it("shows a tool_search query in its label and expanded arguments", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            limit: 20,
            offset: 0,
            query: "browser Playwright navigate click screenshot",
            source: null,
          },
          summary: '{"matches":[]}',
          toolName: "tool_search",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      'Searched "browser Playwright navigate click screenshot"',
    );
    expect(screen.queryByTestId("tool-row-args")).toBeNull();

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-args").textContent).toContain(
      '"query": "browser Playwright navigate click screenshot"',
    );
    expect(screen.getByTestId("tool-row-result").textContent).toContain(
      '"matches":[]',
    );
  });

  it("uses connector-specific labels for search, describe, call, and code", () => {
    expect(
      buildFlatLabel(buildTool({
        args: { source: "playwright" },
        toolName: "tool_search",
      })),
    ).toBe("Listed playwright tools");
    expect(
      buildFlatLabel(buildTool({
        args: { names: ["mcp__playwright__browser_click"] },
        toolName: "tool_describe",
      })),
    ).toBe("Described 1 tool");
    expect(
      buildFlatLabel(buildTool({
        args: {
          arguments: { selector: "#submit" },
          name: "mcp__playwright__browser_click",
        },
        toolName: "tool_call",
      })),
    ).toBe("Called browser_click");
    expect(buildFlatLabel(buildTool({ toolName: "tool_run_code" }))).toBe(
      "Ran connector code",
    );
  });

  it("shows only a tool_call's inner MCP arguments", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            arguments: { selector: "#submit" },
            name: "mcp__playwright__browser_click",
          },
          summary: '{"ok":true}',
          toolName: "tool_call",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    const args = screen.getByTestId("tool-row-args").textContent ?? "";
    expect(args).toContain('"selector": "#submit"');
    expect(args).not.toContain("mcp__playwright__browser_click");
  });

  it("shows arguments for an unrecognized tool", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { foo: "bar" },
          summary: '{"ok":true}',
          toolName: "mystery_tool",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "mystery tool",
    );
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-args").textContent).toContain(
      '"foo": "bar"',
    );
  });

  it("shows running connector arguments before a result arrives", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            arguments: { selector: "#submit" },
            name: "mcp__playwright__browser_click",
          },
          status: "running",
          summary: undefined,
          toolName: "tool_call",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-args").textContent).toContain(
      '"selector": "#submit"',
    );
  });

  it("bounds generic tool arguments to keep the transcript compact", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { query: "x".repeat(5_000) },
          summary: '{"matches":[]}',
          toolName: "tool_search",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    const args = screen.getByTestId("tool-row-args").textContent ?? "";
    expect(args.length).toBeLessThanOrEqual(4_000);
    expect(args).toContain("… (arguments truncated)");
  });

  it("keeps dedicated tool cards free of generic arguments", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/README.md" },
          summary: "file contents",
          toolName: "read",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.queryByTestId("tool-row-args")).toBeNull();

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/plan.plan.md", plan_id: "plan-1" },
          summary: '{"applied":1}',
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.queryByTestId("tool-row-args")).toBeNull();
  });

  it("ask_question renders an always-visible answer card", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            questions: [
              {
                id: "style",
                options: [
                  { id: "run-gun", label: "Run-and-gun", recommended: true },
                ],
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
    expect(screen.getByTestId("answer-option-style").textContent).toContain(
      "Run-and-gun",
    );
  });

  it("renders update_plan checked progress with a View Plan action", () => {
    const onOpenPlanFile = vi.fn();
    render(
      <ToolRow
        item={buildTool({
          args: {
            ops: [
              { kind: "set_status", status: "completed", todo_id: "todo-1" },
              { kind: "set_status", status: "completed", todo_id: "todo-2" },
            ],
            path: "/workspace/login-refactor.plan.md",
            plan_id: "plan-1",
          },
          planActivity: {
            applied: 2,
            checked: 2,
            completed: 4,
            kind: "update",
            total: 9,
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          summary: '{"applied":2}',
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={onOpenPlanFile}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Checked 2 · 4/9",
    );
    expect(screen.getByTestId("view-plan").className).toContain(
      "tc-tool-row__action-link--plan",
    );
    expect(
      screen
        .getByTestId("view-plan")
        .querySelector(".tc-tool-row__action-link-text")?.textContent,
    ).toBe("View Plan");
    expect(
      screen
        .getByTestId("view-plan")
        .querySelector(".tc-tool-row__action-link-chevron")?.className,
    ).toContain("codicon-chevron-right");
    fireEvent.click(screen.getByTestId("view-plan"));
    expect(onOpenPlanFile).toHaveBeenCalledWith(
      "/workspace/login-refactor.plan.md",
    );
  });

  it("renders update_plan state transitions without inventing missing data", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            path: "/workspace/login-refactor.plan.md",
            plan_id: "plan-1",
          },
          planActivity: {
            completed: 8,
            kind: "update",
            stateAfter: "executing",
            stateBefore: "planning",
            total: 9,
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Plan: planning → executing · 8/9",
    );
  });

  it("renders update_plan edit and fallback labels distinctly", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: {
            path: "/workspace/login-refactor.plan.md",
            plan_id: "plan-1",
          },
          planActivity: {
            applied: 3,
            checked: 0,
            completed: 6,
            kind: "update",
            total: 9,
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Updated plan · 6/9",
    );

    rerender(
      <ToolRow
        item={buildTool({
          args: {
            path: "/workspace/login-refactor.plan.md",
            plan_id: "plan-1",
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Updated plan",
    );
    expect(screen.getByTestId("tool-row-label").textContent).not.toContain(
      "/9",
    );
  });

  it("keeps running update_plan rows lightweight and hides View Plan until complete", () => {
    render(
      <ToolRow
        item={buildTool({
          args: {
            path: "/workspace/login-refactor.plan.md",
            plan_id: "plan-1",
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          status: "streaming",
          summary: undefined,
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Updating plan",
    );
    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeTruthy();
    expect(screen.queryByTestId("view-plan")).toBeNull();
    expect(screen.queryByTestId("tool-row-running-indicator")).toBeNull();
  });

  it("keeps failed update_plan rows visible for debugging", () => {
    render(
      <ToolRow
        item={buildTool({
          isError: true,
          summary: "Unable to update plan",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "update_plan failed",
    );
    expect(screen.getByTestId("tool-row-body").textContent).toContain(
      "Unable to update plan",
    );
  });

  it("toolCategory maps built-ins into the new buckets", () => {
    expect(toolCategory("edit")).toBe("edit");
    expect(toolCategory("bash")).toBe("command");
    expect(toolCategory("ask_question")).toBe("answer");
    expect(toolCategory("task_output")).toBe("task");
    expect(toolCategory("task_stop")).toBe("task");
    expect(toolCategory("task_list")).toBe("task");
    expect(toolCategory("read")).toBe("context");
    expect(toolCategory("create_plan")).toBe("other");
    expect(toolCategory("unknown_tool")).toBe("other");
  });

  it("treats only blocking task_output waits as action tools", () => {
    expect(
      isActionTool(
        buildTool({
          args: { block: true, task_id: "task-1", wait_ms: 10_000 },
          status: "running",
          toolName: "task_output",
        }),
      ),
    ).toBe(true);
    expect(
      isActionTool(
        buildTool({
          args: { block: false, task_id: "task-1", wait_ms: 0 },
          toolName: "task_output",
        }),
      ),
    ).toBe(false);
    expect(
      isActionTool(
        buildTool({
          args: { block: true, task_id: "task-1", wait_ms: 10_000 },
          status: "complete",
          toolName: "task_output",
        }),
      ),
    ).toBe(false);
    expect(
      isActionTool(
        buildTool({
          args: { block: true, task_id: "task-1", wait_ms: 0 },
          toolName: "task_output",
        }),
      ),
    ).toBe(false);
    expect(
      isActionTool(
        buildTool({
          args: { task_id: "task-1" },
          toolName: "task_stop",
        }),
      ),
    ).toBe(false);
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

    expect(
      screen.getByTestId("tool-row").getAttribute("data-tool-category"),
    ).toBe("context");
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Searched workspace for config",
    );
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
  });

  it("applies shimmer to running context rows and removes it after completion", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { query: "config" },
          status: "running",
          summary: "Found 1 result.\nconfig.ts:1",
          toolName: "search_workspace",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { query: "config" },
          status: "complete",
          summary: "Found 1 result.\nconfig.ts:1",
          toolName: "search_workspace",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeNull();
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Loaded skill sdk",
    );
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Listed /workspace/src",
    );
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Updated config log.level",
    );
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
    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeTruthy();
    expect(screen.queryByTestId("tool-row-running-indicator")).toBeNull();
  });

  it("applies shimmer to running disclosure-card headers and removes it after completion", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { command: "npm run build" },
          status: "running",
          summary: "building…",
          summaryTitle: null,
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").className).toContain(
      "tc-loading-shimmer",
    );

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/a.rs" },
          diff: [
            { newLine: 1, oldLine: 1, tag: "ctx", text: "fn main() {" },
            { newLine: null, oldLine: 2, tag: "del", text: "  old();" },
            { newLine: 2, oldLine: null, tag: "add", text: "  new();" },
          ],
          display: { file: "/workspace/a.rs", kind: "file" },
          status: "running",
          summary: "editing file",
          toolName: "edit",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeTruthy();

    rerender(
      <ToolRow
        item={buildTool({
          args: { command: "npm run build" },
          status: "complete",
          summary: "built ok",
          summaryTitle: "Build the project",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").className).not.toContain(
      "tc-loading-shimmer",
    );
  });

  it("keeps background bash cards in a running state until the task finishes", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { command: "sleep 12", run_in_background: true },
          backgroundRunning: true,
          backgroundTaskId: "task-1",
          status: "complete",
          summary: '{"taskId":"task-1"}',
          summaryTitle: "Sleep in background",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe(
      "Running in background",
    );
    expect(screen.getByTestId("tool-row-cmd-purpose").className).toContain(
      "tc-loading-shimmer",
    );
    expect(screen.getByTestId("disclosure-card").className).toContain(
      "tc-disclosure-card--running",
    );

    rerender(
      <ToolRow
        item={buildTool({
          args: { command: "sleep 12", run_in_background: true },
          backgroundExitCode: 23,
          backgroundRunning: false,
          backgroundTaskId: "task-1",
          status: "complete",
          summary: '{"taskId":"task-1"}',
          summaryTitle: "Sleep in background",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe(
      "Ran · exit 23",
    );
    expect(screen.getByTestId("tool-row-cmd-purpose").className).not.toContain(
      "tc-loading-shimmer",
    );
    expect(screen.getByTestId("disclosure-card").className).toContain(
      "tc-disclosure-card--success",
    );
  });

  it("renders a task_output countdown row that ticks each second and flips to past tense", () => {
    vi.useFakeTimers();
    const startedAt = new Date("2026-07-21T07:00:00.000Z");
    vi.setSystemTime(startedAt);
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-1", wait_ms: 10000 },
          startedAt: startedAt.getTime(),
          status: "running",
          summary: undefined,
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row").getAttribute("data-tool-category"),
    ).toBe("task");
    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Waiting up to 10s for shell");
    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Waiting up to 9s for shell");

    rerender(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-1", wait_ms: 10000 },
          startedAt: startedAt.getTime(),
          status: "complete",
          summary: '{"wakeReason":"wait_window_elapsed"}',
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Waited for shell");
    expect(
      screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer"),
    ).toBeNull();
  });

  it("renders compact task_output countdown labels and falls back for non-blocking output reads", () => {
    vi.useFakeTimers();
    const now = new Date("2026-07-21T07:00:00.000Z");
    vi.setSystemTime(now);
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-2", wait_ms: 600000 },
          startedAt: now.getTime() - 1000,
          status: "running",
          summary: undefined,
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row").getAttribute("data-tool-category"),
    ).toBe("task");
    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Waiting up to 9m59s for shell");

    rerender(
      <ToolRow
        item={buildTool({
          args: { block: false, task_id: "task-2", wait_ms: 0 },
          status: "complete",
          summary: '{"finished":false}',
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Read output task-2");
  });

  it("renders interrupted task_output rows in past-tense stop wording", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-3", wait_ms: 5000 },
          startedAt: Date.now(),
          status: "interrupted",
          summary: "[interrupted]",
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(
      screen.getByTestId("tool-row-task-output-countdown").textContent,
    ).toBe("Stopped waiting for shell");
  });

  it("formats countdown boundaries with shared helpers", () => {
    expect(clampTaskOutputBudget(undefined)).toBe(5000);
    expect(clampTaskOutputBudget(0)).toBe(0);
    expect(clampTaskOutputBudget(1)).toBe(5000);
    expect(clampTaskOutputBudget(600001)).toBe(600000);
    expect(formatCountdown(599000)).toBe("9m59s");
    expect(formatCountdown(45000)).toBe("45s");
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

    expect(screen.getByTestId("answer-card-question").textContent).toContain(
      "Deploy where?",
    );
    expect(
      screen.getByTestId("answer-option-deploy_target").textContent,
    ).toContain("Staging");
  });

  it.each([
    ["omitted", undefined, false],
    ["null", null, false],
    ["false", false, false],
    ["true", true, true],
  ] as const)(
    "normalizes a %s ask_question recommendation from the transcript",
    (_label, recommended, showsRecommended) => {
      render(
        <ToolRow
          item={buildTool({
            args: {
              questions: [
                {
                  id: "recommendation",
                  options: [{ id: "option", label: "Option", recommended }],
                  prompt: "Choose an option",
                },
              ],
            },
            summary: JSON.stringify({
              answers: [
                {
                  option_ids: ["option"],
                  picked_recommended: showsRecommended,
                  question_id: "recommendation",
                },
              ],
              cancelled: false,
            }),
            toolName: "ask_question",
          })}
          onOpenFile={vi.fn()}
        />,
      );

      expect(screen.getByTestId("answer-card-question").textContent).toContain(
        "Choose an option",
      );
      if (showsRecommended) {
        expect(screen.getByText("Recommended")).toBeTruthy();
      } else {
        expect(screen.queryByText("Recommended")).toBeNull();
      }
    },
  );

  it("commandBinaries parses, dedupes and caps command-name tags", () => {
    expect(commandBinaries("git status")).toEqual(["git"]);
    expect(commandBinaries("git status && echo '---' && git log")).toEqual([
      "git",
      "echo",
    ]);
    expect(commandBinaries("cat a | grep foo | sort")).toEqual([
      "cat",
      "grep",
      "sort",
    ]);
    expect(commandBinaries("FOO=bar sudo ./deploy.sh")).toEqual(["deploy.sh"]);
    expect(commandBinaries("/usr/local/bin/node script.js")).toEqual(["node"]);
    expect(commandBinaries("a; b; c; d; e")).toEqual(["a", "b", "c"]);
    expect(
      commandBinaries(
        "cd /tmp\n# generate icon\ncat <<'SVG' > icon.svg\n<svg>\n</svg>\nSVG\nsvgcleaner icon.svg",
      ),
    ).toEqual(["cd", "cat", "svgcleaner"]);
    expect(
      commandBinaries("git status && # comment only\n<svg>\n> out.txt"),
    ).toEqual(["git"]);
    expect(commandBinaries("")).toEqual([]);
    expect(commandBinaries(undefined)).toEqual([]);
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Interrupted edit",
    );
    expect(screen.getByTestId("tool-row-body").textContent).toContain(
      "Interrupted",
    );
  });

  it("shows bounded live shell output, five-line preview, full log, and final summary", () => {
    const onOpenFile = vi.fn();
    const liveOutput = Array.from(
      { length: 510 },
      (_, index) => `line-${index}`,
    ).join("\n");
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          status: "streaming",
          toolName: "bash",
          args: { command: "build" },
          liveOutput: `${liveOutput}\nstdout: compiling\nstderr: warning`,
          logPath: "/tmp/full.log",
          summary: undefined,
        })}
        onOpenFile={onOpenFile}
      />,
    );
    const preview = screen.getByTestId("terminal-output-preview");
    expect(preview.textContent).toContain("line-509");
    expect(preview.textContent).toContain("stderr: warning");
    expect(preview.textContent).not.toContain("line-504");
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "line-509",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).not.toContain(
      "line-0",
    );
    fireEvent.click(screen.getByTestId("tool-row-full-log"));
    expect(onOpenFile).toHaveBeenCalledWith("/tmp/full.log");
    expect(
      screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded"),
    ).toBe("true");

    rerender(
      <ToolRow
        item={buildTool({
          status: "complete",
          toolName: "bash",
          args: { command: "build" },
          liveOutput,
          logPath: "/tmp/full.log",
          summary: "final authoritative summary",
        })}
        onOpenFile={onOpenFile}
      />,
    );
    expect(
      screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded"),
    ).toBe("true");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "final authoritative summary",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).not.toContain(
      "stderr: warning",
    );
  });

  it("batch edit card summarises applied and failed files and keeps failures visible", () => {
    render(
      <ToolRow
        item={buildTool({
          display: {
            files: [
              {
                added: 18,
                file: "/workspace/provider.ts",
                removed: 4,
                status: "applied",
              },
              {
                file: "/workspace/protocol.ts",
                note: "第 2 段匹配到 3 处",
                status: "failed",
              },
            ],
            kind: "files",
            summary: "1 个文件已落盘，1 个失败且未写入",
          },
          toolName: "edit",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Edited 2 files",
    );
    expect(screen.getByTestId("tool-row-diff-added").textContent).toBe("+18");
    expect(screen.getByTestId("tool-row-diff-removed").textContent).toBe("-4");
    expect(screen.getByTestId("tool-row-files-status").textContent).toBe(
      "1 applied · 1 failed",
    );

    // 有失败项时默认展开：折叠态只有一个计数，看不出是哪个文件、为什么失败。
    const entries = screen.getAllByTestId("tool-row-file-entry");
    expect(entries).toHaveLength(2);
    expect(entries[1].getAttribute("data-status")).toBe("failed");
    expect(screen.getByTestId("tool-row-file-note").textContent).toBe(
      "第 2 段匹配到 3 处",
    );
  });

  it("flat batch read shows per-file ranges and skipped entries", () => {
    render(
      <ToolRow
        item={buildTool({
          display: {
            files: [
              { file: "/workspace/provider.ts", range: "L1-900 (900 lines)" },
              {
                file: "/workspace/state.ts",
                note: "output budget exhausted; resume: read(path=\"/workspace/state.ts\")",
                status: "skipped",
              },
            ],
            kind: "files",
            summary: "已读取 1 个文件，1 个因输出预算跳过",
          },
          toolName: "read",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain(
      "Read 2 files",
    );
    expect(screen.getByTestId("tool-row-files-status").textContent).toBe(
      "1 skipped",
    );

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-file-range").textContent).toBe(
      "L1-900 (900 lines)",
    );
    expect(screen.getByTestId("tool-row-file-note").textContent).toContain(
      "output budget exhausted",
    );
  });

  it.each([
    ["read", "standalone", 1], ["read", "grouped", 2],
    ["read_file", "standalone", 2], ["read_file", "grouped", 1],
  ] as const)("keeps %s files flat in %s (%s entries), without raw body", (toolName, variant, count) => {
    const onOpenFile = vi.fn();
    const item = buildTool({ toolName, summary: count === 1 ? undefined : "RAW_READ_BODY_SENTINEL", display: {
      kind: "files", summary: "", files: Array.from({ length: count }, (_, n) => ({ file: `/workspace/file-${n}.md`, range: "L1-9 (9 lines)" })),
    } });
    const { container } = render(<ToolRow item={item} variant={variant} onOpenFile={onOpenFile} />);
    expect(screen.queryByTestId("disclosure-card")).toBeNull();
    expect(screen.getByTestId("tool-row-label").textContent).toBe(`Read ${count} ${count === 1 ? "file" : "files"}`);
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe("true");
    expect(screen.getAllByTestId("tool-row-file-entry")).toHaveLength(count);
    expect(screen.getAllByTestId("tool-row-file-range")[0].textContent).toBe("L1-9 (9 lines)");
    expect(container.textContent).not.toContain("RAW_READ_BODY_SENTINEL");
    fireEvent.click(screen.getAllByTestId("file-chip")[0]);
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/file-0.md");
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
  });

  it("opens file-level failures even when the batch succeeded, but keeps the user's collapse", () => {
    const item = buildTool({ isError: false, summary: "RAW_READ_BODY_SENTINEL", display: { kind: "files", summary: "", files: [
      { file: "/workspace/missing.md", status: "failed", note: "Permission denied" },
      { file: "/workspace/skipped.md", status: "skipped", note: "output budget exhausted" },
    ] } });
    const { rerender } = render(<ToolRow item={item} onOpenFile={vi.fn()} />);
    expect(screen.queryByTestId("disclosure-card")).toBeNull();
    expect(screen.getByTestId("tool-row-files-status").textContent).toBe("1 failed · 1 skipped");
    expect(screen.getAllByTestId("tool-row-file-note")[0].textContent).toBe("Permission denied");
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    rerender(<ToolRow item={{ ...item, summary: "a later result" }} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
    expect(screen.getByTestId("tool-row-files-status").textContent).toContain("1 failed");
  });

  it.each([false, true])("applies file results after streaming without replacing user choice (interacted=%s)", (interacted) => {
    const initial = buildTool({ status: "streaming", summary: undefined });
    const { rerender } = render(<ToolRow item={initial} onOpenFile={vi.fn()} />);
    const streaming = { ...initial, display: { kind: "files" as const, summary: "", files: [{ file: "/workspace/a.md" }] } };
    rerender(<ToolRow item={streaming} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe("true");
    if (interacted) fireEvent.click(screen.getByTestId("tool-row-toggle"));
    rerender(<ToolRow item={{ ...streaming, status: "complete", display: { kind: "files", summary: "", files: [{ file: "/workspace/a.md", status: "failed", note: "read failed" }] } }} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe(interacted ? "false" : "true");
  });

  it.each([false, true])("collapses a successful streaming read unless the user explicitly opened it (interacted=%s)", (interacted) => {
    const initial = buildTool({ status: "streaming", summary: undefined });
    const { rerender } = render(<ToolRow item={initial} onOpenFile={vi.fn()} />);
    const streaming = { ...initial, display: { kind: "files" as const, summary: "", files: [{ file: "/workspace/a.md" }] } };
    rerender(<ToolRow item={streaming} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe("true");
    if (interacted) {
      fireEvent.click(screen.getByTestId("tool-row-toggle"));
      fireEvent.click(screen.getByTestId("tool-row-toggle"));
    }
    rerender(<ToolRow item={{ ...streaming, status: "complete" }} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-toggle").getAttribute("aria-expanded")).toBe(interacted ? "true" : "false");
  });

  it("does not turn an empty read files result into a raw-output expander", () => {
    render(<ToolRow item={buildTool({ summary: "RAW_READ_BODY_SENTINEL", display: { kind: "files", summary: "", files: [] } })} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId("tool-row-label").textContent).toBe("Read 0 files");
    expect(screen.queryByTestId("tool-row-toggle")).toBeNull();
    expect(screen.queryByTestId("tool-row-body")).toBeNull();
    expect(screen.queryByTestId("disclosure-card")).toBeNull();
  });

  it("batch card lets each file open its own diff", () => {
    render(
      <ToolRow
        item={buildTool({
          display: {
            files: [
              {
                added: 1,
                diff: [
                  { newLine: 1, oldLine: null, tag: "add", text: "added()" },
                ],
                file: "/workspace/a.rs",
                removed: 0,
                status: "applied",
              },
            ],
            kind: "files",
            summary: "已编辑 1 个文件，全部落盘",
          },
          toolName: "edit",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.queryByText("added()")).toBeNull();
    fireEvent.click(screen.getByTestId("tool-row-file-toggle"));
    expect(screen.getByText("added()")).toBeTruthy();
  });
});
