import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WebviewToolCard } from "../types";
import {
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Edited");
    expect(screen.getByTestId("tool-row-diff-added").textContent).toBe("+4");
    expect(screen.getByTestId("tool-row-diff-removed").textContent).toBe("-2");
    expect(screen.getByTestId("tool-row-open-diff")).toBeTruthy();
    expect(screen.getByTestId("tool-row-open-diff").textContent).toContain("View diff");
    expect(screen.getByTestId("tool-row-open-diff").className).not.toContain(
      "tc-tool-row__action-link--plan",
    );
    expect(
      screen.getByTestId("tool-row-open-diff").querySelector(".tc-tool-row__action-link-chevron"),
    ).toBeNull();
    expect(container.querySelector(".tc-tool-row__leading-icon")).toBeNull();
    expect(screen.getByTestId("disclosure-card-leading-icon")).toBeTruthy();
    expect(screen.getByTestId("diff-view-preview").closest(".tc-disclosure-card")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tool-row-open-diff"));
    expect(onOpenDiff).toHaveBeenCalledWith("tc-1");
    expect(screen.queryByRole("button", { name: /apply/i })).toBeNull();
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
    const preview = screen.getByTestId("terminal-output-preview").textContent ?? "";
    expect(preview).toContain("$ cargo test");
    expect(preview).not.toContain("line 1");
    expect(preview).toContain("line 2");
    expect(preview).toContain("line 6");
    fireEvent.click(screen.getByTestId("tool-row-toggle"));
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("$ cargo test");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("line 1");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("line 6");
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
    expect(screen.getByTestId("tool-row-cmd-tags").textContent).toBe("git, echo");
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

  it("renders a completed create_plan as a pinned plan card", () => {
    const onBuildPlan = vi.fn();
    const onOpenPlanFile = vi.fn();
    render(
      <ToolRow
        availableModels={["gpt-5.4"]}
        buildModel="gpt-5.4"
        canBuildPlan
        currentPlanId="plan-1"
        currentPlanState="planning"
        item={buildTool({
          args: {
            goal: "Login refactor plan",
            path: "/workspace/login-refactor.plan.md",
            todos: [
              { content: "Audit the transcript path", id: "todo-1", status: "completed" },
              { content: "Render update_plan events", id: "todo-2", status: "in_progress" },
            ],
          },
          planActivity: {
            completed: 1,
            kind: "create",
            stateAfter: "planning",
            title: "Login refactor plan",
            total: 2,
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          summary: "{\"plan_id\":\"plan-1\",\"path\":\"/workspace/login-refactor.plan.md\",\"state\":\"planning\"}",
          toolName: "create_plan",
        })}
        onBuildPlan={onBuildPlan}
        onOpenFile={vi.fn()}
        onOpenPlanFile={onOpenPlanFile}
        planTodos={[
          { content: "Audit the transcript path", id: "todo-1", status: "completed" },
          { content: "Render update_plan events", id: "todo-2", status: "in_progress" },
        ]}
      />,
    );

    expect(screen.getByTestId("plan-card-title").textContent).toBe("Login refactor plan");
    expect(screen.getByTestId("plan-card-file-name").textContent).toBe("login-refactor.plan.md");
    expect(screen.getByTestId("plan-todos-count").textContent).toBe("2 todos");
    expect((screen.getByTestId("build-plan") as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(screen.getByTestId("view-plan"));
    expect(onOpenPlanFile).toHaveBeenCalledWith("/workspace/login-refactor.plan.md");

    fireEvent.click(screen.getByTestId("build-plan"));
    expect(onBuildPlan).toHaveBeenCalledWith("plan-1", "/workspace/login-refactor.plan.md");
  });

  it("renders a running create_plan as the legacy pending plan card", () => {
    const { rerender } = render(
      <ToolRow
        availableModels={["gpt-5.4"]}
        buildModel="gpt-5.4"
        canBuildPlan
        item={buildTool({
          args: {
            draft: "Keep one create card and many update rows.",
            goal: "Login refactor plan",
            todos: [
              { content: "Audit the transcript path", id: "todo-1", status: "completed" },
              { content: "Render update_plan events", id: "todo-2", status: "pending" },
            ],
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          status: "running",
          toolName: "create_plan",
        })}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("plan-card-title").textContent).toBe("Login refactor plan");
    expect(screen.getByTestId("plan-todos-count").textContent).toBe("2 todos");
    expect((screen.getByTestId("view-plan-pending") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getAllByTestId("plan-card")).toHaveLength(1);
    expect(screen.queryByTestId("tool-row-label")).toBeNull();

    rerender(
      <ToolRow
        availableModels={["gpt-5.4"]}
        buildModel="gpt-5.4"
        canBuildPlan
        item={buildTool({
          args: {
            draft: "Keep one create card and many update rows.",
            goal: "Login refactor plan",
            todos: [
              { content: "Audit the transcript path", id: "todo-1", status: "completed" },
              { content: "Render update_plan events", id: "todo-2", status: "pending" },
            ],
          },
          planActivity: {
            completed: 1,
            kind: "create",
            stateAfter: "planning",
            title: "Login refactor plan",
            total: 2,
          },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          status: "complete",
          summary:
            "{\"plan_id\":\"plan-1\",\"path\":\"/workspace/login-refactor.plan.md\",\"state\":\"planning\"}",
          toolName: "create_plan",
        })}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("view-plan-pending")).toBeNull();
    expect(screen.getByTestId("view-plan").textContent).toBe("View Plan");
    expect(screen.getAllByTestId("plan-card")).toHaveLength(1);
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
          summary: "{\"applied\":2}",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={onOpenPlanFile}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Checked 2 · 4/9");
    expect(screen.getByTestId("view-plan").className).toContain("tc-tool-row__action-link--plan");
    expect(
      screen.getByTestId("view-plan").querySelector(".tc-tool-row__action-link-text")?.textContent,
    ).toBe("View Plan");
    expect(
      screen.getByTestId("view-plan").querySelector(".tc-tool-row__action-link-chevron")?.className,
    ).toContain("codicon-chevron-right");
    fireEvent.click(screen.getByTestId("view-plan"));
    expect(onOpenPlanFile).toHaveBeenCalledWith("/workspace/login-refactor.plan.md");
  });

  it("renders update_plan state transitions without inventing missing data", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/login-refactor.plan.md", plan_id: "plan-1" },
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
          args: { path: "/workspace/login-refactor.plan.md", plan_id: "plan-1" },
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Updated plan · 6/9");

    rerender(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/login-refactor.plan.md", plan_id: "plan-1" },
          planId: "plan-1",
          planPath: "/workspace/login-refactor.plan.md",
          toolName: "update_plan",
        })}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Updated plan");
    expect(screen.getByTestId("tool-row-label").textContent).not.toContain("/9");
  });

  it("keeps running update_plan rows lightweight and hides View Plan until complete", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { path: "/workspace/login-refactor.plan.md", plan_id: "plan-1" },
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain("Updating plan");
    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeTruthy();
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

    expect(screen.getByTestId("tool-row-label").textContent).toContain("update_plan failed");
    expect(screen.getByTestId("tool-row-body").textContent).toContain("Unable to update plan");
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
          args: { block: true, task_id: "task-1", timeout_ms: 10_000 },
          status: "running",
          toolName: "task_output",
        }),
      ),
    ).toBe(true);
    expect(
      isActionTool(
        buildTool({
          args: { block: false, task_id: "task-1", timeout_ms: 0 },
          toolName: "task_output",
        }),
      ),
    ).toBe(false);
    expect(
      isActionTool(
        buildTool({
          args: { block: true, task_id: "task-1", timeout_ms: 10_000 },
          status: "complete",
          toolName: "task_output",
        }),
      ),
    ).toBe(false);
    expect(
      isActionTool(
        buildTool({
          args: { block: true, task_id: "task-1", timeout_ms: 0 },
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

    expect(screen.getByTestId("tool-row").getAttribute("data-tool-category")).toBe("context");
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

    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeTruthy();

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

    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeNull();
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
    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeTruthy();
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

    expect(screen.getByTestId("tool-row-cmd-purpose").className).toContain("tc-loading-shimmer");

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

    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeTruthy();

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

    expect(screen.getByTestId("tool-row-cmd-purpose").className).not.toContain("tc-loading-shimmer");
  });

  it("keeps background bash cards in a running state until the task finishes", () => {
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { command: "sleep 12", run_in_background: true },
          backgroundRunning: true,
          backgroundTaskId: "task-1",
          status: "complete",
          summary: "{\"taskId\":\"task-1\"}",
          summaryTitle: "Sleep in background",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe("Running in background");
    expect(screen.getByTestId("tool-row-cmd-purpose").className).toContain("tc-loading-shimmer");
    expect(screen.getByTestId("disclosure-card").className).toContain("tc-disclosure-card--running");

    rerender(
      <ToolRow
        item={buildTool({
          args: { command: "sleep 12", run_in_background: true },
          backgroundExitCode: 23,
          backgroundRunning: false,
          backgroundTaskId: "task-1",
          status: "complete",
          summary: "{\"taskId\":\"task-1\"}",
          summaryTitle: "Sleep in background",
          toolName: "bash",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-cmd-purpose").textContent).toBe("Ran · exit 23");
    expect(screen.getByTestId("tool-row-cmd-purpose").className).not.toContain("tc-loading-shimmer");
    expect(screen.getByTestId("disclosure-card").className).toContain("tc-disclosure-card--success");
  });

  it("renders a task_output countdown row that ticks each second and flips to past tense", () => {
    vi.useFakeTimers();
    const startedAt = new Date("2026-07-21T07:00:00.000Z");
    vi.setSystemTime(startedAt);
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-1", timeout_ms: 10000 },
          startedAt: startedAt.getTime(),
          status: "running",
          summary: undefined,
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row").getAttribute("data-tool-category")).toBe("task");
    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Waiting up to 10s for shell",
    );
    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Waiting up to 9s for shell",
    );

    rerender(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-1", timeout_ms: 10000 },
          startedAt: startedAt.getTime(),
          status: "complete",
          summary: "{\"wakeReason\":\"timeout\"}",
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Waited for shell",
    );
    expect(screen.getByTestId("tool-row-label").querySelector(".tc-loading-shimmer")).toBeNull();
  });

  it("renders compact task_output countdown labels and falls back for non-blocking output reads", () => {
    vi.useFakeTimers();
    const now = new Date("2026-07-21T07:00:00.000Z");
    vi.setSystemTime(now);
    const { rerender } = render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-2", timeout_ms: 600000 },
          startedAt: now.getTime() - 1000,
          status: "running",
          summary: undefined,
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row").getAttribute("data-tool-category")).toBe("task");
    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Waiting up to 9m59s for shell",
    );

    rerender(
      <ToolRow
        item={buildTool({
          args: { block: false, task_id: "task-2", timeout_ms: 0 },
          status: "complete",
          summary: "{\"finished\":false}",
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Read output task-2",
    );
  });

  it("renders interrupted task_output rows in past-tense stop wording", () => {
    render(
      <ToolRow
        item={buildTool({
          args: { block: true, task_id: "task-3", timeout_ms: 5000 },
          startedAt: Date.now(),
          status: "interrupted",
          summary: "[interrupted]",
          toolName: "task_output",
        })}
        onOpenFile={vi.fn()}
      />,
    );

    expect(screen.getByTestId("tool-row-task-output-countdown").textContent).toBe(
      "Stopped waiting for shell",
    );
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

    expect(screen.getByTestId("answer-card-question").textContent).toContain("Deploy where?");
    expect(screen.getByTestId("answer-option-deploy_target").textContent).toContain("Staging");
  });

  it("commandBinaries parses, dedupes and caps command-name tags", () => {
    expect(commandBinaries("git status")).toEqual(["git"]);
    expect(commandBinaries("git status && echo '---' && git log")).toEqual(["git", "echo"]);
    expect(commandBinaries("cat a | grep foo | sort")).toEqual(["cat", "grep", "sort"]);
    expect(commandBinaries("FOO=bar sudo ./deploy.sh")).toEqual(["deploy.sh"]);
    expect(commandBinaries("/usr/local/bin/node script.js")).toEqual(["node"]);
    expect(commandBinaries("a; b; c; d; e")).toEqual(["a", "b", "c"]);
    expect(
      commandBinaries("cd /tmp\n# generate icon\ncat <<'SVG' > icon.svg\n<svg>\n</svg>\nSVG\nsvgcleaner icon.svg"),
    ).toEqual(["cd", "cat", "svgcleaner"]);
    expect(commandBinaries("git status && # comment only\n<svg>\n> out.txt")).toEqual(["git"]);
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Interrupted edit");
    expect(screen.getByTestId("tool-row-body").textContent).toContain("Interrupted");
  });
});
