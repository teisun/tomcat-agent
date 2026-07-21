import { useEffect, useMemo, useState, type ReactNode } from "react";

import type {
  AskQuestionResult,
  WebviewApprovalQuestion,
  WebviewPlanFileCard,
  WebviewPlanState,
  WebviewTodo,
  WebviewToolCard,
} from "../types";
import { AnswerCard } from "./AnswerCard";
import { DiffView } from "./DiffView";
import { DisclosureCard, type DisclosureStatusVariant } from "./DisclosureCard";
import { FileChip } from "./FileChip";
import { PlanFileCard } from "./PlanFileCard";
import { tailTerminalOutput, TerminalOutput } from "./TerminalOutput";

function firstLine(value: string | undefined): string | undefined {
  if (!value) {
    return undefined;
  }
  return value.split("\n").find((line) => line.trim())?.trim();
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function humanizeToolName(toolName: string): string {
  return toolName.replace(/_/g, " ");
}

export type ToolCategory = "answer" | "command" | "context" | "edit" | "other";

const TASK_OUTPUT_BLOCK_DEFAULT_TIMEOUT_MS = 5_000;
const TASK_OUTPUT_BLOCK_MIN_TIMEOUT_MS = 5_000;
const TASK_OUTPUT_BLOCK_MAX_TIMEOUT_MS = 600_000;

const EDIT_TOOLS = new Set(["edit", "hashline_edit", "write"]);
const COMMAND_TOOLS = new Set(["bash", "execute_command", "shell"]);
const ANSWER_TOOLS = new Set(["ask_question"]);
const CONTEXT_TOOLS = new Set([
  "grep",
  "list_dir",
  "load_skill",
  "read",
  "read_file",
  "search_files",
  "search_workspace",
  "web_fetch",
  "web_search",
]);
const OTHER_TOOLS = new Set(["config_get", "config_set", "create_plan", "todos", "update_plan"]);

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  return normalized.split("/").pop() || filePath;
}

function filePathForTool(item: WebviewToolCard): string | undefined {
  const args = item.args ?? {};
  return item.display?.kind === "file" ? item.display.file : asString(args.path);
}

function isPlanTool(item: WebviewToolCard): boolean {
  return item.toolName === "create_plan" || item.toolName === "update_plan";
}

function planPathForTool(item: WebviewToolCard): string | undefined {
  return item.planPath ?? asString(item.args?.path);
}

function createPlanTodosFromArgs(args: Record<string, unknown> | undefined): WebviewTodo[] | undefined {
  const todos = args?.todos;
  if (!Array.isArray(todos)) {
    return undefined;
  }
  const parsed = todos.flatMap((todo, index) => {
    if (typeof todo !== "object" || todo === null) {
      return [];
    }
    const entry = todo as Record<string, unknown>;
    const content = asString(entry.content) ?? `Todo ${index + 1}`;
    const id = asString(entry.id) ?? `todo-${index + 1}`;
    const status =
      entry.status === "cancelled" ||
      entry.status === "completed" ||
      entry.status === "in_progress" ||
      entry.status === "pending"
        ? entry.status
        : "pending";
    return [{ content, id, status } satisfies WebviewTodo];
  });
  return parsed.length > 0 ? parsed : undefined;
}

function createPlanCardFromTool(
  item: WebviewToolCard,
  options: {
    currentPlanId?: string | null;
    currentPlanState?: WebviewPlanState | null;
    planTodos?: WebviewTodo[];
  },
): WebviewPlanFileCard | null {
  if (item.toolName !== "create_plan" || item.isError) {
    return null;
  }
  const creating = isRunning(item);
  if (!creating && item.status !== "complete") {
    return null;
  }
  const path = planPathForTool(item);
  if (!path) {
    return null;
  }
  const isActivePlan = !!item.planId && item.planId === options.currentPlanId;
  const argTodos = createPlanTodosFromArgs(item.args);
  const ambientTodos = options.planTodos && options.planTodos.length > 0 ? options.planTodos : undefined;
  return {
    id: item.id,
    overview: item.planActivity?.overview ?? undefined,
    path,
    planId: item.planId ?? null,
    state: isActivePlan ? options.currentPlanState ?? item.planActivity?.stateAfter ?? null : item.planActivity?.stateAfter ?? null,
    title: item.planActivity?.title ?? asString(item.args?.goal) ?? undefined,
    todos: isActivePlan ? ambientTodos ?? argTodos : argTodos,
    type: "plan",
  };
}

export function isRunning(item: WebviewToolCard): boolean {
  return (item.status === "running" || item.status === "streaming") && !item.isError;
}

export function formatCountdown(ms: number): string {
  const totalSeconds = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes > 0) {
    return `${minutes}m${seconds.toString().padStart(2, "0")}s`;
  }
  return `${totalSeconds}s`;
}

export function clampTaskOutputBudget(value: unknown): number {
  if (value === 0) {
    return 0;
  }
  const raw = asNumber(value) ?? TASK_OUTPUT_BLOCK_DEFAULT_TIMEOUT_MS;
  return Math.min(
    TASK_OUTPUT_BLOCK_MAX_TIMEOUT_MS,
    Math.max(TASK_OUTPUT_BLOCK_MIN_TIMEOUT_MS, raw),
  );
}

function formatToolSummary(summary: string | undefined): string | undefined {
  if (!summary) {
    return undefined;
  }
  return summary.trim() === "[interrupted]" ? "Interrupted" : summary;
}

export function toolCategory(toolName: string): ToolCategory {
  if (EDIT_TOOLS.has(toolName)) {
    return "edit";
  }
  if (COMMAND_TOOLS.has(toolName)) {
    return "command";
  }
  if (ANSWER_TOOLS.has(toolName)) {
    return "answer";
  }
  if (CONTEXT_TOOLS.has(toolName)) {
    return "context";
  }
  if (OTHER_TOOLS.has(toolName)) {
    return "other";
  }
  return "other";
}

export function isActionTool(item: WebviewToolCard): boolean {
  if (isPlanTool(item)) {
    return true;
  }
  const category = toolCategory(item.toolName);
  return category === "answer" || category === "command" || category === "edit";
}

function buildPlanUpdateLabel(item: WebviewToolCard): string {
  if (isRunning(item)) {
    return "Updating plan";
  }
  const activity = item.planActivity;
  if (!activity || activity.kind !== "update") {
    return "Updated plan";
  }
  const hasProgress =
    typeof activity.completed === "number" && typeof activity.total === "number";
  const progressSuffix = hasProgress ? ` · ${activity.completed}/${activity.total}` : "";
  if (
    activity.stateBefore &&
    activity.stateAfter &&
    activity.stateBefore !== activity.stateAfter
  ) {
    return `Plan: ${activity.stateBefore} → ${activity.stateAfter}${progressSuffix}`;
  }
  if ((activity.checked ?? 0) > 0) {
    return hasProgress
      ? `Checked ${activity.checked} · ${activity.completed}/${activity.total}`
      : `Checked ${activity.checked}`;
  }
  if ((activity.applied ?? 0) > 0) {
    return hasProgress ? `Updated plan · ${activity.completed}/${activity.total}` : "Updated plan";
  }
  return "Updated plan";
}

function countResults(summary: string | undefined): number | null {
  if (!summary) {
    return null;
  }
  const match = summary.match(/Found\s+(\d+)\s+results?/i);
  if (match) {
    return Number(match[1]);
  }
  const hits = summary
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !/^found\s+\d+\s+results?\.?$/i.test(line));
  return hits.length > 0 ? hits.length : null;
}

function toolIconClass(toolName: string): string {
  switch (toolName) {
    case "write":
      return "codicon-new-file";
    case "edit":
    case "hashline_edit":
      return "codicon-diff-modified";
    case "ask_question":
      return "codicon-question";
    case "read":
    case "read_file":
      return "codicon-eye";
    case "load_skill":
      return "codicon-book";
    case "grep":
    case "search_files":
    case "web_search":
    case "search_workspace":
      return "codicon-search";
    case "bash":
    case "execute_command":
    case "shell":
      return "codicon-terminal";
    case "web_fetch":
      return "codicon-globe";
    case "list_dir":
      return "codicon-folder";
    case "config_get":
    case "config_set":
      return "codicon-gear";
    case "create_plan":
    case "update_plan":
    case "todos":
      return "codicon-list-tree";
    default:
      return "codicon-tools";
  }
}

export function buildFlatLabel(item: WebviewToolCard): string {
  const args = item.args ?? {};
  const running = isRunning(item);
  const category = toolCategory(item.toolName);

  if (item.status === "interrupted") {
    switch (category) {
      case "edit":
        return item.toolName === "write" ? "Interrupted write" : "Interrupted edit";
      case "command":
        return "Interrupted command";
      case "answer":
        return "Interrupted question";
      default:
        return `Interrupted ${humanizeToolName(item.toolName)}`;
    }
  }

  if (item.isError && isPlanTool(item)) {
    return `${item.toolName} failed`;
  }

  switch (item.toolName) {
    case "read":
    case "read_file":
      return running ? "Reading file" : "Read file";
    case "load_skill": {
      const name = asString(args.name) ?? "skill";
      return running ? `Loading skill ${name}` : `Loaded skill ${name}`;
    }
    case "grep": {
      const query = asString(args.pattern) ?? asString(args.query) ?? "pattern";
      return running ? `Searching ${query}` : `Searched ${query}`;
    }
    case "search_files": {
      const query = asString(args.pattern) ?? asString(args.query) ?? asString(args.path) ?? "files";
      return running ? `Searching files for ${query}` : `Searched files for ${query}`;
    }
    case "bash": {
      const command = firstLine(asString(args.command)) ?? "command";
      return running ? `Running ${command}` : `Ran ${command}`;
    }
    case "task_output": {
      if (args.block === true && !item.isError && clampTaskOutputBudget(args.timeout_ms) > 0) {
        if (item.status === "interrupted") {
          return "Stopped waiting for shell";
        }
        return running ? "Waiting for shell" : "Waited for shell";
      }
      const taskId = asString(args.task_id) ?? "task";
      return running ? `Reading output ${taskId}` : `Read output ${taskId}`;
    }
    case "task_stop": {
      const taskId = asString(args.task_id) ?? "task";
      return running ? `Stopping ${taskId}` : `Stopped ${taskId}`;
    }
    case "task_list":
      return running ? "Listing tasks" : "Listed tasks";
    case "list_dir": {
      const dir = asString(args.path) ?? "directory";
      return running ? `Listing ${dir}` : `Listed ${dir}`;
    }
    case "web_search": {
      const query = asString(args.query) ?? "query";
      return running ? `Searching "${query}"` : `Searched "${query}"`;
    }
    case "search_workspace": {
      const query = asString(args.query) ?? asString(args.pattern);
      if (query) {
        return running ? `Searching workspace for ${query}` : `Searched workspace for ${query}`;
      }
      return running ? "Searching workspace" : "Searched workspace";
    }
    case "web_fetch": {
      const url = asString(args.url) ?? "url";
      return running ? `Fetching ${url}` : `Fetched ${url}`;
    }
    case "config_get": {
      const key = asString(args.key) ?? "config";
      return running ? `Reading config ${key}` : `Read config ${key}`;
    }
    case "config_set": {
      const key = asString(args.key) ?? "config";
      return running ? `Updating config ${key}` : `Updated config ${key}`;
    }
    case "create_plan": {
      return running ? "Creating plan" : "Created plan";
    }
    case "update_plan": {
      return buildPlanUpdateLabel(item);
    }
    case "todos":
      return running ? "Updating todos" : "Updated todos";
    case "ask_question":
      return running ? "Asking question" : "Asked question";
    case "edit":
    case "hashline_edit":
      return running ? "Editing file" : "Edited file";
    case "write":
      return running ? "Creating file" : "Created file";
    default:
      return `${humanizeToolName(item.toolName)}${running ? "…" : ""}`;
  }
}

export function buildGroupTitleFromTool(item: WebviewToolCard): string {
  const filePath = filePathForTool(item);
  if (filePath && (item.toolName === "read" || item.toolName === "read_file")) {
    return `${buildFlatLabel(item)} ${basename(filePath)}`;
  }
  if (filePath && toolCategory(item.toolName) === "edit") {
    return `${buildFlatLabel(item)} ${basename(filePath)}`;
  }
  return buildFlatLabel(item);
}

export function buildToolCollectionTitle(tools: WebviewToolCard[]): string {
  if (tools.length === 0) {
    return "Thinking";
  }
  if (tools.length === 1) {
    return buildGroupTitleFromTool(tools[0]);
  }

  if (tools.every((tool) => tool.toolName === "read" || tool.toolName === "read_file")) {
    return `Reviewed ${tools.length} files`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "context")) {
    return `Searched ${tools.length} sources`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "command")) {
    return tools.length === 1 ? buildGroupTitleFromTool(tools[0]) : `Executed ${tools.length} commands`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "edit")) {
    return `Edited ${tools.length} files`;
  }

  return `Used ${tools.length} tools`;
}

function loadingTextClass(active: boolean): string {
  return active ? " tc-loading-shimmer" : "";
}

function isBlockingTaskOutput(item: WebviewToolCard): boolean {
  return item.toolName === "task_output" && item.args?.block === true && !item.isError;
}

function taskOutputCountdownLabel(
  item: WebviewToolCard,
  nowTick: number,
): string | null {
  if (!isBlockingTaskOutput(item)) {
    return null;
  }
  const budget = clampTaskOutputBudget(item.args?.timeout_ms);
  if (budget === 0) {
    return null;
  }
  if (item.status === "interrupted") {
    return "Stopped waiting for shell";
  }
  if (!isRunning(item)) {
    return "Waited for shell";
  }
  const startedAt = asNumber(item.startedAt) ?? nowTick;
  const elapsed = Math.max(0, nowTick - startedAt);
  const remaining = Math.max(0, budget - elapsed);
  return `Waiting up to ${formatCountdown(remaining)} for shell`;
}

export function hasMeaningfulContent(item: WebviewToolCard): boolean {
  if (isPlanTool(item) && !item.isError) {
    return false;
  }
  const summary = formatToolSummary(item.summary);
  if (
    toolCategory(item.toolName) === "edit" &&
    item.status === "complete" &&
    !item.isError
  ) {
    if ((item.diff?.length ?? 0) > 0) {
      return true;
    }
    return Boolean(
      item.diffStat && (item.diffStat.added > 0 || item.diffStat.removed > 0),
    );
  }
  if (
    item.toolName === "ask_question" &&
    parseApprovalQuestions(item.args) &&
    parseAskQuestionResult(item.summary)
  ) {
    return true;
  }
  if (summary?.trim()) {
    return true;
  }
  if (item.display?.kind === "plan") {
    return Boolean(item.display.plan.trim());
  }
  if (item.display?.kind === "text") {
    return Boolean(item.display.text.trim());
  }
  return false;
}

function commandText(item: WebviewToolCard): string {
  const args = item.args ?? {};
  return (
    firstLine(asString(args.command)) ??
    firstLine(asString(args.cmd)) ??
    firstLine(asString(args.script)) ??
    "command"
  );
}

/** 完整命令串（多行/多段保留），用于终端正文 `$ …` 提示行。 */
function fullCommandText(item: WebviewToolCard): string {
  const args = item.args ?? {};
  return asString(args.command) ?? asString(args.cmd) ?? asString(args.script) ?? "";
}

/**
 * 客户端解析命令名标签（零 LLM）：按 `&& || | ; \n` 切段，取每段首个"可执行名"，
 * 剔除注释、heredoc 正文、`VAR=…` 环境赋值与 `sudo`，去掉路径前缀，去重、上限 3 个。
 * 例：`git status && echo '---'` → `["git", "echo"]`。
 */
const COMMAND_NAME_RE = /^[A-Za-z_][A-Za-z0-9_.+-]*$/u;
const ENV_ASSIGNMENT_RE = /^[A-Za-z_][A-Za-z0-9_]*=/u;
const HEREDOC_OPEN_RE = /<<-?\s*(['"]?)([^'"`\s;|&<>]+)\1/u;

function heredocTerminator(line: string): string | null {
  return line.match(HEREDOC_OPEN_RE)?.[2] ?? null;
}

function firstCommandName(segment: string): string | null {
  const trimmed = segment.trim();
  if (!trimmed || trimmed.startsWith("#")) {
    return null;
  }
  let binary: string | undefined;
  for (const token of trimmed.split(/\s+/)) {
    if (!token) {
      continue;
    }
    if (ENV_ASSIGNMENT_RE.test(token)) {
      continue;
    }
    if (token === "sudo" || token === "command") {
      continue;
    }
    binary = token;
    break;
  }
  if (!binary) {
    return null;
  }
  const name = binary.replace(/^\.\//u, "").split("/").pop() ?? "";
  return COMMAND_NAME_RE.test(name) ? name : null;
}

export function commandBinaries(command: string | undefined): string[] {
  if (!command || !command.trim()) {
    return [];
  }
  const names: string[] = [];
  let heredocEnd: string | null = null;
  for (const line of command.split("\n")) {
    const trimmedLine = line.trim();
    if (heredocEnd) {
      if (trimmedLine === heredocEnd) {
        heredocEnd = null;
      }
      continue;
    }
    if (!trimmedLine || trimmedLine.startsWith("#")) {
      continue;
    }
    for (const segment of line.split(/&&|\|\||[|;]/)) {
      const name = firstCommandName(segment);
      if (name && !names.includes(name)) {
        names.push(name);
      }
      if (names.length >= 3) {
        return names;
      }
    }
    heredocEnd = heredocTerminator(line);
  }
  return names;
}

/** bash 卡片头的占位动词（summaryTitle 未到时）：中断/运行中/已完成三态。 */
function commandPlaceholderVerb(item: WebviewToolCard): string {
  if (item.status === "interrupted") {
    return "Interrupted";
  }
  return isRunning(item) ? "Running" : "Ran";
}

function parseApprovalQuestions(args: Record<string, unknown> | undefined): WebviewApprovalQuestion[] | null {
  const questions = args?.questions;
  if (!Array.isArray(questions)) {
    return null;
  }
  const parsed = questions.filter(
    (question): question is WebviewApprovalQuestion =>
      typeof question === "object" &&
      question !== null &&
      typeof question.id === "string" &&
      typeof question.prompt === "string" &&
      Array.isArray(question.options) &&
      question.options.every(
        (option) =>
          typeof option === "object" &&
          option !== null &&
          typeof option.id === "string" &&
          typeof option.label === "string",
      ),
  );
  return parsed.length === questions.length ? parsed : null;
}

function parseAskQuestionResult(summary: string | undefined): AskQuestionResult | null {
  if (!summary) {
    return null;
  }
  try {
    const parsed = JSON.parse(summary) as {
      answers?: unknown[];
      cancelled?: unknown;
    };
    if (!parsed || typeof parsed !== "object" || !Array.isArray(parsed.answers)) {
      return null;
    }
    if (typeof parsed.cancelled !== "boolean") {
      return null;
    }
    const answers = parsed.answers.map((entry) => {
      if (!entry || typeof entry !== "object") {
        return null;
      }
      const answer = entry as Record<string, unknown>;
      const questionId =
        typeof answer.questionId === "string"
          ? answer.questionId
          : typeof answer.question_id === "string"
            ? answer.question_id
            : null;
      const optionIds = Array.isArray(answer.optionIds)
        ? answer.optionIds
        : Array.isArray(answer.option_ids)
          ? answer.option_ids
          : null;
      const pickedRecommended =
        typeof answer.pickedRecommended === "boolean"
          ? answer.pickedRecommended
          : typeof answer.picked_recommended === "boolean"
            ? answer.picked_recommended
            : null;
      const customText =
        typeof answer.customText === "string" || answer.customText === null
          ? answer.customText
          : typeof answer.custom_text === "string" || answer.custom_text === null
            ? answer.custom_text
            : undefined;
      const skipped =
        typeof answer.skipped === "boolean" ? answer.skipped : undefined;
      if (
        !questionId ||
        !optionIds ||
        !optionIds.every((optionId) => typeof optionId === "string") ||
        typeof pickedRecommended !== "boolean"
      ) {
        return null;
      }
      return {
        customText: customText ?? undefined,
        optionIds,
        pickedRecommended,
        questionId,
        skipped,
      };
    });
    if (answers.some((entry) => entry === null)) {
      return null;
    }
    return {
      answers: answers.filter((entry): entry is AskQuestionResult["answers"][number] => entry !== null),
      cancelled: parsed.cancelled,
    };
  } catch {
    return null;
  }
}

function renderPlanActionLink(
  path: string | undefined,
  onOpenPlanFile: ((path: string) => void) | undefined,
): ReactNode {
  if (!path || !onOpenPlanFile) {
    return null;
  }
  return (
    <button
      className="tc-tool-row__action-link tc-tool-row__action-link--plan"
      data-testid="view-plan"
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onOpenPlanFile(path);
      }}
      type="button"
    >
      <span className="tc-tool-row__action-link-text">View Plan</span>
      <span
        aria-hidden="true"
        className="codicon codicon-chevron-right tc-tool-row__action-link-chevron"
      />
    </button>
  );
}

function renderPlainBody(item: WebviewToolCard): ReactNode {
  return (
    <>
      {item.summary ? <pre data-testid="tool-row-result">{formatToolSummary(item.summary)}</pre> : null}
      {item.display?.kind === "plan" ? <pre>{item.display.plan}</pre> : null}
      {item.display?.kind === "text" && item.display.text !== item.summary ? (
        <pre>{item.display.text}</pre>
      ) : null}
    </>
  );
}

function renderFlatContent(
  item: WebviewToolCard,
  onOpenFile: (path: string) => void,
  onOpenPlanFile?: (path: string) => void,
  nowTick?: number,
): ReactNode {
  const args = item.args ?? {};
  const filePath = filePathForTool(item);
  const planPath = planPathForTool(item);
  const category = toolCategory(item.toolName);
  const diffStat = item.diffStat;
  const textClassName = `tc-tool-row__text${loadingTextClass(isRunning(item))}`;

  switch (category) {
    case "edit":
      if (filePath) {
        return (
          <span className="tc-tool-row__inline">
            <span className={textClassName}>{buildFlatLabel(item).replace(/ file$/, "")}</span>
            <FileChip onOpenFile={onOpenFile} path={filePath} />
            {diffStat ? (
              <span className="tc-tool-row__diff-badges" data-testid="tool-row-diff-badges">
                <span
                  className="tc-tool-row__diff-badge tc-tool-row__diff-badge--added"
                  data-testid="tool-row-diff-added"
                >
                  +{diffStat.added}
                </span>
                <span
                  className="tc-tool-row__diff-badge tc-tool-row__diff-badge--removed"
                  data-testid="tool-row-diff-removed"
                >
                  -{diffStat.removed}
                </span>
              </span>
            ) : null}
          </span>
        );
      }
      return <span className={textClassName}>{buildFlatLabel(item)}</span>;
    case "command": {
      // Flat rows have no terminal body to host the command, so keep the command
      // visible inline; the async summaryTitle (when present) leads as the purpose.
      return (
        <span className="tc-tool-row__inline">
          <span className={textClassName} data-testid="tool-row-cmd-purpose">
            {asString(item.summaryTitle) ?? commandPlaceholderVerb(item)}
          </span>
          <code className="tc-tool-row__cmd" data-testid="tool-row-cmd">
            {commandText(item)}
          </code>
        </span>
      );
    }
    case "answer":
      return <span className={textClassName}>{buildFlatLabel(item)}</span>;
    case "context":
    case "other":
      switch (item.toolName) {
        case "task_output": {
          const countdownLabel = nowTick === undefined ? null : taskOutputCountdownLabel(item, nowTick);
          return (
            <span className={textClassName} data-testid="tool-row-task-output-countdown">
              {countdownLabel ?? buildFlatLabel(item)}
            </span>
          );
        }
        case "create_plan":
        case "update_plan":
          return (
            <span className="tc-tool-row__inline">
              <span className={textClassName}>{buildFlatLabel(item)}</span>
              {isRunning(item) || item.isError ? null : renderPlanActionLink(planPath, onOpenPlanFile)}
            </span>
          );
        case "grep": {
          const resultsCount = countResults(item.summary);
          const suffix =
            !isRunning(item) && resultsCount ? ` · ${resultsCount} results` : "";
          const glob = asString(args.glob) ?? asString(args.path);
          if (glob) {
            return (
              <span className="tc-tool-row__inline">
                <span className={textClassName}>
                  {buildFlatLabel(item)}
                  {suffix}
                </span>
                <FileChip onOpenFile={onOpenFile} path={glob} />
              </span>
            );
          }
          return <span className={textClassName}>{`${buildFlatLabel(item)}${suffix}`}</span>;
        }
        case "read":
        case "read_file":
          if (filePath) {
            return (
              <span className="tc-tool-row__inline">
                <span className={textClassName}>{buildFlatLabel(item).replace(/ file$/, "")}</span>
                <FileChip onOpenFile={onOpenFile} path={filePath} />
              </span>
            );
          }
          return <span className={textClassName}>{buildFlatLabel(item)}</span>;
        default:
          return <span className={textClassName}>{buildFlatLabel(item)}</span>;
      }
    default:
      return <span className={textClassName}>{buildFlatLabel(item)}</span>;
  }
}

function renderExpandedBody(item: WebviewToolCard): ReactNode {
  const category = toolCategory(item.toolName);
  if (category === "answer") {
    const questions = parseApprovalQuestions(item.args);
    const result = parseAskQuestionResult(item.summary);
    if (questions && result) {
      return <AnswerCard questions={questions} result={result} />;
    }
    return renderPlainBody(item);
  }

  if (category === "command") {
    return (
      <div
        className={`tc-tool-row__terminal${item.isError ? " tc-tool-row__terminal--error" : item.status === "complete" ? " tc-tool-row__terminal--success" : " tc-tool-row__terminal--running"}`}
        data-testid="tool-row-terminal"
      >
        {renderPlainBody(item)}
      </div>
    );
  }

  if (item.toolName === "web_search" && item.summary) {
    const lines = item.summary
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line && !/^found\s+\d+\s+results?\.?$/i.test(line));
    if (lines.length) {
      return (
        <ul className="tc-tool-row__hits" data-testid="tool-row-hits">
          {lines.map((line, index) => (
            <li key={`${line}-${index}`}>{line}</li>
          ))}
        </ul>
      );
    }
  }

  return renderPlainBody(item);
}

function renderDiffBadges(item: WebviewToolCard): ReactNode {
  if (!item.diffStat) {
    return null;
  }
  return (
    <span className="tc-tool-row__diff-badges" data-testid="tool-row-diff-badges">
      <span
        className="tc-tool-row__diff-badge tc-tool-row__diff-badge--added"
        data-testid="tool-row-diff-added"
      >
        +{item.diffStat.added}
      </span>
      <span
        className="tc-tool-row__diff-badge tc-tool-row__diff-badge--removed"
        data-testid="tool-row-diff-removed"
      >
        -{item.diffStat.removed}
      </span>
    </span>
  );
}

function shouldShowBodyByDefault(
  item: WebviewToolCard,
  contentVisible: boolean,
): boolean {
  if (!contentVisible) {
    return false;
  }
  if (toolCategory(item.toolName) === "answer") {
    return true;
  }
  return item.isError || item.status !== "complete";
}

export function ToolRow({
  availableModels = [],
  buildModel = "",
  canBuildPlan = false,
  currentPlanId = null,
  currentPlanState = null,
  item,
  onBuildPlan,
  onOpenFile,
  onOpenDiff,
  onOpenPlanFile,
  onSetBuildModel,
  planTodos = [],
  variant = "standalone",
}: {
  availableModels?: string[];
  buildModel?: string;
  canBuildPlan?: boolean;
  currentPlanId?: string | null;
  currentPlanState?: WebviewPlanState | null;
  item: WebviewToolCard;
  onBuildPlan?(planId: string | null, path: string): void;
  onOpenFile(path: string): void;
  onOpenDiff?(toolCallId: string): void;
  onOpenPlanFile?(path: string): void;
  onSetBuildModel?(modelId: string): void;
  planTodos?: WebviewTodo[];
  variant?: "grouped" | "standalone";
}) {
  const category = toolCategory(item.toolName);
  const contentVisible = hasMeaningfulContent(item);
  const alwaysVisibleBody = category === "answer" && contentVisible;
  const canToggle = contentVisible && !alwaysVisibleBody;
  const shouldExpandByDefault = shouldShowBodyByDefault(item, contentVisible);
  const [collapsed, setCollapsed] = useState(!shouldExpandByDefault);
  const [nowTick, setNowTick] = useState(() => Date.now());
  const [userInteracted, setUserInteracted] = useState(false);
  const countdownActive =
    isRunning(item) &&
    isBlockingTaskOutput(item) &&
    clampTaskOutputBudget(item.args?.timeout_ms) > 0;

  useEffect(() => {
    setCollapsed(!shouldExpandByDefault);
    setUserInteracted(false);
  }, [item.id, shouldExpandByDefault]);

  useEffect(() => {
    if (!userInteracted) {
      setCollapsed(!shouldExpandByDefault);
    }
  }, [shouldExpandByDefault, userInteracted]);

  useEffect(() => {
    setNowTick(Date.now());
    if (!countdownActive) {
      return;
    }
    const intervalId = window.setInterval(() => {
      setNowTick(Date.now());
    }, 1000);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [countdownActive, item.id, item.startedAt]);

  const createPlanCard = useMemo(
    () =>
      createPlanCardFromTool(item, {
        currentPlanId,
        currentPlanState,
        planTodos,
      }),
    [currentPlanId, currentPlanState, item, planTodos],
  );
  const iconClass = useMemo(() => toolIconClass(item.toolName), [item.toolName]);
  if (createPlanCard) {
    return (
      <PlanFileCard
        availableModels={availableModels}
        buildModel={buildModel}
        canBuild={canBuildPlan}
        creating={isRunning(item)}
        item={createPlanCard}
        onBuild={(planId, path) => onBuildPlan?.(planId, path)}
        onOpenPlanFile={(path) => onOpenPlanFile?.(path)}
        onSetBuildModel={onSetBuildModel}
        planTodos={planTodos}
      />
    );
  }
  const shellClassName =
    variant === "grouped"
      ? "tc-tool-row-shell tc-tool-row-shell--grouped tc-thinking-tool-wrapper"
      : "tc-tool-row-shell tc-tool-row-shell--standalone";
  const iconNode =
    variant === "grouped" ? (
      <span
        aria-hidden="true"
        className={`tc-thinking-icon codicon ${iconClass}`}
      />
    ) : (
      <span
        aria-hidden="true"
        className={`tc-tool-row__leading-icon codicon ${iconClass}`}
      />
    );
  const disclosureLeadingIcon = (
    <span
      aria-hidden="true"
      className={`tc-disclosure-card__leading-icon codicon ${iconClass}`}
    />
  );
  const hasStructuredDiff = (item.diff?.length ?? 0) > 0;
  const hasLargeDiffFallback =
    category === "edit" &&
    item.display?.kind === "file" &&
    !hasStructuredDiff &&
    Boolean(item.diffStat && (item.diffStat.added > 0 || item.diffStat.removed > 0));
  const usesDisclosureCard =
    (category === "command" && contentVisible) ||
    (category === "edit" && item.display?.kind === "file" && (hasStructuredDiff || hasLargeDiffFallback));
  const disclosureStatusVariant: DisclosureStatusVariant = item.isError
    ? "error"
    : item.status === "complete"
      ? "success"
      : "running";
  const showOpenDiffButton =
    category === "edit" &&
    item.display?.kind === "file" &&
    hasStructuredDiff &&
    Boolean(item.toolCallId) &&
    Boolean(onOpenDiff);

  const disclosureHeader = (
    <div className="tc-tool-row__card-header">
      <span className="tc-tool-row__inline" data-testid="tool-row-label">
        {category === "command" ? (
          <>
            <span
              className={`tc-tool-row__text${loadingTextClass(isRunning(item))}`}
              data-testid="tool-row-cmd-purpose"
            >
              {asString(item.summaryTitle) ?? commandPlaceholderVerb(item)}
            </span>
            {commandBinaries(fullCommandText(item)).length > 0 ? (
              <span className="tc-tool-row__cmd-tags" data-testid="tool-row-cmd-tags">
                {commandBinaries(fullCommandText(item)).join(", ")}
              </span>
            ) : null}
          </>
        ) : (
          <>
            <span className={`tc-tool-row__text${loadingTextClass(isRunning(item))}`}>
              {buildFlatLabel(item).replace(/ file$/, "")}
            </span>
            {item.display?.kind === "file" ? (
              <FileChip onOpenFile={onOpenFile} path={item.display.file} />
            ) : null}
            {renderDiffBadges(item)}
          </>
        )}
      </span>
      {showOpenDiffButton ? (
        <button
          aria-label="View diff"
          className="tc-tool-row__action-link"
          data-testid="tool-row-open-diff"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onOpenDiff?.(item.toolCallId);
          }}
          type="button"
        >
          <span aria-hidden="true" className="codicon codicon-diff" />
          <span>View diff</span>
        </button>
      ) : null}
    </div>
  );

  return (
    <div className={shellClassName} data-testid="tool-row-wrapper">
      {usesDisclosureCard ? null : iconNode}
      <div
        className={`tc-tool-row tc-tool-row--${category}${item.isError ? " tc-tool-row--error" : ""}`}
        data-testid="tool-row"
        data-tool-category={category}
        data-tool-variant={variant}
      >
        {usesDisclosureCard ? (
          <DisclosureCard
            bodyTestId="tool-row-body"
            defaultExpanded={shouldExpandByDefault}
            header={disclosureHeader}
            leadingIcon={disclosureLeadingIcon}
            preview={
              category === "command" ? (
                <TerminalOutput
                  command={fullCommandText(item)}
                  preview
                  text={tailTerminalOutput(item.summary, 5)}
                />
              ) : (
                <DiffView diff={item.diff} previewRows={5} />
              )
            }
            resetKey={item.id}
            statusVariant={disclosureStatusVariant}
            toggleTestId="tool-row-toggle"
          >
            {category === "command" ? (
              <TerminalOutput command={fullCommandText(item)} text={item.summary} />
            ) : (
              <DiffView diff={item.diff} />
            )}
          </DisclosureCard>
        ) : (
          <>
            <div className="tc-tool-row__header">
              <span className="tc-tool-row__label" data-testid="tool-row-label">
                {renderFlatContent(item, onOpenFile, onOpenPlanFile, nowTick)}
              </span>
              {canToggle ? (
                <button
                  aria-expanded={!collapsed}
                  aria-label={collapsed ? "Expand tool result" : "Collapse tool result"}
                  className="tc-tool-row__toggle"
                  data-testid="tool-row-toggle"
                  onClick={() => {
                    setUserInteracted(true);
                    setCollapsed((value) => !value);
                  }}
                  type="button"
                >
                  <span className="tc-tool-row__caret">{collapsed ? "▸" : "▾"}</span>
                </button>
              ) : null}
            </div>
            {collapsed || !contentVisible ? null : (
              <div className="tc-tool-row__body" data-testid="tool-row-body">
                {renderExpandedBody(item)}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
