import { memo, useEffect, useMemo, useState, type ReactNode } from "react";
import { isDiffViewable } from "../../../src/shared/diffPresentation";

import type {
  AskQuestionAnswer,
  AskQuestionResult,
  WebviewApprovalOption,
  WebviewApprovalQuestion,
  WebviewToolCard,
  WebviewToolDisplayFileEntry,
} from "../types";
import { AnswerCard } from "./AnswerCard";
import { DiffView } from "./DiffView";
import { DisclosureCard, type DisclosureStatusVariant } from "./DisclosureCard";
import { FileChip } from "./FileChip";
import {
  limitTerminalOutput,
  tailTerminalOutput,
  TerminalOutput,
} from "./TerminalOutput";

function firstLine(value: string | undefined): string | undefined {
  if (!value) {
    return undefined;
  }
  return value
    .split("\n")
    .find((line) => line.trim())
    ?.trim();
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.every((entry: unknown): entry is string => typeof entry === "string")
  );
}

type TranscriptApprovalOption = Omit<WebviewApprovalOption, "recommended"> & {
  recommended?: boolean | null;
};

type TranscriptApprovalQuestion = Omit<WebviewApprovalQuestion, "options"> & {
  options: TranscriptApprovalOption[];
};

function isApprovalOption(value: unknown): value is TranscriptApprovalOption {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.label === "string" &&
    (value.recommended === undefined ||
      value.recommended === null ||
      typeof value.recommended === "boolean")
  );
}

function isApprovalQuestion(value: unknown): value is TranscriptApprovalQuestion {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.prompt === "string" &&
    Array.isArray(value.options) &&
    value.options.every(isApprovalOption)
  );
}

function humanizeToolName(toolName: string): string {
  return toolName.replace(/_/g, " ");
}

function connectorToolShortName(name: string): string {
  if (!name.startsWith("mcp__")) {
    return name;
  }
  const separatorIndex = name.indexOf("__", "mcp__".length);
  return separatorIndex === -1 ? name : name.slice(separatorIndex + 2);
}

export type ToolCategory =
  "answer" | "command" | "context" | "edit" | "other" | "task";

const TASK_OUTPUT_BLOCK_DEFAULT_TIMEOUT_MS = 5_000;
const TASK_OUTPUT_BLOCK_MIN_TIMEOUT_MS = 5_000;
const TASK_OUTPUT_BLOCK_MAX_TIMEOUT_MS = 600_000;
const GENERIC_TOOL_ARGS_MAX_CHARS = 4_000;
const GENERIC_TOOL_ARGS_TRUNCATED_SUFFIX = "\n… (arguments truncated)";

const EDIT_TOOLS = new Set(["edit", "hashline_edit", "write"]);
const COMMAND_TOOLS = new Set(["bash", "execute_command", "shell"]);
const ANSWER_TOOLS = new Set(["ask_question"]);
const TASK_TOOLS = new Set(["task_output", "task_stop", "task_list"]);
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
const OTHER_TOOLS = new Set([
  "config_get",
  "config_set",
  "create_plan",
  "todos",
  "update_plan",
]);
// These tools already have dedicated labels, icons, or expanded cards. Every
// other tool uses the generic card so new connector tools cannot hide inputs.
const DEDICATED_CARD_TOOLS = new Set([
  ...EDIT_TOOLS,
  ...COMMAND_TOOLS,
  ...ANSWER_TOOLS,
  ...TASK_TOOLS,
  ...CONTEXT_TOOLS,
  ...OTHER_TOOLS,
]);

function usesGenericToolCard(toolName: string): boolean {
  return !DEDICATED_CARD_TOOLS.has(toolName);
}

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  return normalized.split("/").pop() || filePath;
}

function filePathForTool(item: WebviewToolCard): string | undefined {
  const args = item.args ?? {};
  return item.display?.kind === "file"
    ? item.display.file
    : asString(args.path);
}

function isPlanTool(item: WebviewToolCard): boolean {
  return item.toolName === "create_plan" || item.toolName === "update_plan";
}

function planPathForTool(item: WebviewToolCard): string | undefined {
  return item.planPath ?? asString(item.args?.path);
}

export function isRunning(item: WebviewToolCard): boolean {
  return (
    (item.status === "running" || item.status === "streaming") && !item.isError
  );
}

function isRunningForDisplay(item: WebviewToolCard): boolean {
  return isRunning(item) || item.backgroundRunning === true;
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

function takeUnicodeSafePrefix(text: string, maxCodeUnits: number): string {
  if (text.length <= maxCodeUnits) {
    return text;
  }
  let end = maxCodeUnits;
  const previous = text.charCodeAt(end - 1);
  const next = text.charCodeAt(end);
  if (
    previous >= 0xd800 &&
    previous <= 0xdbff &&
    next >= 0xdc00 &&
    next <= 0xdfff
  ) {
    end -= 1;
  }
  return text.slice(0, end);
}

function formatToolArgsForDisplay(item: WebviewToolCard): string | undefined {
  if (!usesGenericToolCard(item.toolName)) {
    return undefined;
  }
  const rawArgs =
    item.toolName === "tool_call" ? item.args?.arguments : item.args;
  if (
    !isRecord(rawArgs) ||
    Array.isArray(rawArgs) ||
    Object.keys(rawArgs).length === 0
  ) {
    return undefined;
  }
  const serialized = JSON.stringify(rawArgs, null, 2);
  if (!serialized) {
    return undefined;
  }
  if (serialized.length <= GENERIC_TOOL_ARGS_MAX_CHARS) {
    return serialized;
  }
  return `${takeUnicodeSafePrefix(
    serialized,
    GENERIC_TOOL_ARGS_MAX_CHARS - GENERIC_TOOL_ARGS_TRUNCATED_SUFFIX.length,
  )}${GENERIC_TOOL_ARGS_TRUNCATED_SUFFIX}`;
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
  if (TASK_TOOLS.has(toolName)) {
    return "task";
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
  if (isRunning(item) && isBlockingTaskOutput(item)) {
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
    typeof activity.completed === "number" &&
    typeof activity.total === "number";
  const progressSuffix = hasProgress
    ? ` · ${activity.completed}/${activity.total}`
    : "";
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
    return hasProgress
      ? `Updated plan · ${activity.completed}/${activity.total}`
      : "Updated plan";
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

export function toolIconClass(toolName: string): string {
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
    case "tool_search":
    case "web_search":
    case "search_workspace":
      return "codicon-search";
    case "bash":
    case "execute_command":
    case "shell":
      return "codicon-terminal";
    case "task_output":
      return "codicon-watch";
    case "task_stop":
      return "codicon-debug-stop";
    case "task_list":
      return "codicon-tools";
    case "tool_describe":
      return "codicon-book";
    case "tool_call":
      return "codicon-plug";
    case "tool_run_code":
      return "codicon-code";
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
        return item.toolName === "write"
          ? "Interrupted write"
          : "Interrupted edit";
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
      const query =
        asString(args.pattern) ??
        asString(args.query) ??
        asString(args.path) ??
        "files";
      return running
        ? `Searching files for ${query}`
        : `Searched files for ${query}`;
    }
    case "bash": {
      const backgroundLabel = backgroundCommandLabel(item);
      if (backgroundLabel) {
        return backgroundLabel;
      }
      const command = commandText(item);
      return running ? `Running ${command}` : `Ran ${command}`;
    }
    case "task_output":
      return taskOutputStateLabel(item);
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
    case "tool_search": {
      const query = asString(args.query);
      if (query) {
        return running ? `Searching "${query}"` : `Searched "${query}"`;
      }
      const source = asString(args.source);
      if (source) {
        return running ? `Listing ${source} tools` : `Listed ${source} tools`;
      }
      return running ? "Listing connectors" : "Listed connectors";
    }
    case "tool_describe": {
      const names = Array.isArray(args.names)
        ? args.names.filter((name): name is string => typeof name === "string")
        : [];
      const target = names.length === 1 ? "1 tool" : `${names.length} tools`;
      return running ? `Describing ${target}` : `Described ${target}`;
    }
    case "tool_call": {
      const name = asString(args.name);
      const target = name ? connectorToolShortName(name) : "connector tool";
      return running ? `Calling ${target}` : `Called ${target}`;
    }
    case "tool_run_code":
      return running ? "Running connector code" : "Ran connector code";
    case "search_workspace": {
      const query = asString(args.query) ?? asString(args.pattern);
      if (query) {
        return running
          ? `Searching workspace for ${query}`
          : `Searched workspace for ${query}`;
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

  if (
    tools.every(
      (tool) => tool.toolName === "read" || tool.toolName === "read_file",
    )
  ) {
    return `Reviewed ${tools.length} files`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "context")) {
    return `Searched ${tools.length} sources`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "command")) {
    return tools.length === 1
      ? buildGroupTitleFromTool(tools[0])
      : `Executed ${tools.length} commands`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "edit")) {
    return `Edited ${tools.length} files`;
  }
  if (tools.every((tool) => toolCategory(tool.toolName) === "task")) {
    return tools.length === 1
      ? buildGroupTitleFromTool(tools[0])
      : "Managed background tasks";
  }

  return `Used ${tools.length} tools`;
}

function loadingTextClass(active: boolean): string {
  return active ? " tc-loading-shimmer" : "";
}

function isBlockingTaskOutput(item: WebviewToolCard): boolean {
  return (
    item.toolName === "task_output" &&
    item.args?.block === true &&
    !item.isError &&
    clampTaskOutputBudget(item.args?.wait_ms) > 0
  );
}

function taskOutputStateLabel(item: WebviewToolCard): string {
  const running = isRunning(item);
  if (isBlockingTaskOutput(item)) {
    if (item.status === "interrupted") {
      return "Stopped waiting for shell";
    }
    return running ? "Waiting for shell" : "Waited for shell";
  }
  const taskId = asString(item.args?.task_id) ?? "task";
  return running ? `Reading output ${taskId}` : `Read output ${taskId}`;
}

function taskOutputCountdownLabel(
  item: WebviewToolCard,
  nowTick: number,
): string | null {
  if (!isBlockingTaskOutput(item)) {
    return null;
  }
  const budget = clampTaskOutputBudget(item.args?.wait_ms);
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
  if (item.liveOutput?.trim()) {
    return true;
  }
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

function shellQuoteArg(value: string): string {
  if (value === "") {
    return "''";
  }
  if (/^[A-Za-z0-9_@%+=:,./-]+$/u.test(value)) {
    return value;
  }
  return `'${value.replace(/'/gu, `'\\''`)}'`;
}

/** 完整命令串（多行/多段保留），用于终端正文 `$ …` 提示行。 */
function fullCommandText(item: WebviewToolCard): string {
  const toolArgs = item.args ?? {};
  const command =
    asString(toolArgs.command) ??
    asString(toolArgs.cmd) ??
    asString(toolArgs.script) ??
    "";
  const argv = Array.isArray(toolArgs.args)
    ? toolArgs.args.filter((arg): arg is string => typeof arg === "string")
    : [];
  if (!command || argv.length === 0) {
    return command;
  }
  return `${command} ${argv.map(shellQuoteArg).join(" ")}`;
}

function commandText(item: WebviewToolCard): string {
  return firstLine(fullCommandText(item)) ?? "command";
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
  const backgroundLabel = backgroundCommandLabel(item);
  if (backgroundLabel) {
    return backgroundLabel;
  }
  if (item.status === "interrupted") {
    return "Interrupted";
  }
  return isRunning(item) ? "Running" : "Ran";
}

function backgroundCommandLabel(item: WebviewToolCard): string | null {
  const isBackgroundCommand =
    (item.toolName === "bash" ||
      item.toolName === "shell" ||
      item.toolName === "execute_command") &&
    (item.backgroundRunning === true ||
      typeof item.backgroundTaskId === "string");
  if (!isBackgroundCommand) {
    return null;
  }
  if (item.backgroundRunning === true) {
    return "Running in background";
  }
  if (
    typeof item.backgroundExitCode === "number" &&
    item.backgroundExitCode !== 0
  ) {
    return `Ran · exit ${item.backgroundExitCode}`;
  }
  return "Ran";
}

function commandPurposeLabel(item: WebviewToolCard): string {
  return (
    backgroundCommandLabel(item) ??
    asString(item.summaryTitle) ??
    commandPlaceholderVerb(item)
  );
}

function parseApprovalQuestions(
  args: Record<string, unknown> | undefined,
): WebviewApprovalQuestion[] | null {
  const questions = args?.questions;
  if (!Array.isArray(questions)) {
    return null;
  }
  const parsed = questions.filter(isApprovalQuestion);
  return parsed.length === questions.length
    ? parsed.map((question) => ({
        ...question,
        options: question.options.map((option) => ({
          ...option,
          // Older transcripts may serialize an absent recommendation as null.
          // Normalize at the protocol boundary so renderers only see booleans.
          recommended: option.recommended === true,
        })),
      }))
    : null;
}

function parseAskQuestionResult(
  summary: string | undefined,
): AskQuestionResult | null {
  if (!summary) {
    return null;
  }
  try {
    const parsed = JSON.parse(summary) as {
      answers?: unknown[];
      cancelled?: unknown;
    };
    if (
      !parsed ||
      typeof parsed !== "object" ||
      !Array.isArray(parsed.answers)
    ) {
      return null;
    }
    if (typeof parsed.cancelled !== "boolean") {
      return null;
    }
    const answers: Array<AskQuestionAnswer | null> = parsed.answers.map(
      (entry): AskQuestionAnswer | null => {
      if (!isRecord(entry)) {
        return null;
      }
      const questionId =
        typeof entry.questionId === "string"
          ? entry.questionId
          : typeof entry.question_id === "string"
            ? entry.question_id
            : null;
      const optionIds = isStringArray(entry.optionIds)
        ? entry.optionIds
        : isStringArray(entry.option_ids)
          ? entry.option_ids
          : null;
      const pickedRecommended =
        typeof entry.pickedRecommended === "boolean"
          ? entry.pickedRecommended
          : typeof entry.picked_recommended === "boolean"
            ? entry.picked_recommended
            : null;
      const customText =
        typeof entry.customText === "string" || entry.customText === null
          ? entry.customText
          : typeof entry.custom_text === "string" ||
              entry.custom_text === null
            ? entry.custom_text
            : undefined;
      const skipped =
        typeof entry.skipped === "boolean" ? entry.skipped : undefined;
      if (
        !questionId ||
        !optionIds ||
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
      },
    );
    if (answers.some((entry) => entry === null)) {
      return null;
    }
    return {
      answers: answers.filter(
        (entry): entry is AskQuestionAnswer => entry !== null,
      ),
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
      {item.summary ? (
        <pre data-testid="tool-row-result">
          {formatToolSummary(item.summary)}
        </pre>
      ) : null}
      {item.display?.kind === "plan" ? <pre>{item.display.plan}</pre> : null}
      {item.display?.kind === "text" && item.display.text !== item.summary ? (
        <pre>{item.display.text}</pre>
      ) : null}
    </>
  );
}

function renderToolArgsSection(args: string | undefined): ReactNode {
  return args ? (
    <pre aria-label="Tool arguments" data-testid="tool-row-args">
      {args}
    </pre>
  ) : null;
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
  const textClassName = `tc-tool-row__text${loadingTextClass(isRunningForDisplay(item))}`;

  switch (category) {
    case "edit":
      if (filePath) {
        return (
          <span className="tc-tool-row__inline">
            <span className={textClassName}>
              {buildFlatLabel(item).replace(/ file$/, "")}
            </span>
            <FileChip onOpenFile={onOpenFile} path={filePath} />
            {diffStat ? (
              <span
                className="tc-tool-row__diff-badges"
                data-testid="tool-row-diff-badges"
              >
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
            {commandPurposeLabel(item)}
          </span>
          <code className="tc-tool-row__cmd" data-testid="tool-row-cmd">
            {commandText(item)}
          </code>
        </span>
      );
    }
    case "answer":
      return <span className={textClassName}>{buildFlatLabel(item)}</span>;
    case "task": {
      const countdownLabel =
        nowTick === undefined ? null : taskOutputCountdownLabel(item, nowTick);
      return (
        <span
          className={textClassName}
          data-testid="tool-row-task-output-countdown"
        >
          {countdownLabel ?? buildFlatLabel(item)}
        </span>
      );
    }
    case "context":
    case "other":
      switch (item.toolName) {
        case "create_plan":
        case "update_plan":
          return (
            <span className="tc-tool-row__inline">
              <span className={textClassName}>{buildFlatLabel(item)}</span>
              {isRunning(item) || item.isError
                ? null
                : renderPlanActionLink(planPath, onOpenPlanFile)}
            </span>
          );
        case "grep": {
          const resultsCount = countResults(item.summary);
          const suffix =
            !isRunning(item) && resultsCount
              ? ` · ${resultsCount} results`
              : "";
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
          return (
            <span
              className={textClassName}
            >{`${buildFlatLabel(item)}${suffix}`}</span>
          );
        }
        case "read":
        case "read_file":
          if (filePath) {
            return (
              <span className="tc-tool-row__inline">
                <span className={textClassName}>
                  {buildFlatLabel(item).replace(/ file$/, "")}
                </span>
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

function renderExpandedBody(
  item: WebviewToolCard,
  genericArgs: string | undefined,
): ReactNode {
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

  return (
    <>
      {renderToolArgsSection(genericArgs)}
      {renderPlainBody(item)}
    </>
  );
}

function fileEntryIconClass(
  entry: WebviewToolDisplayFileEntry,
): string | null {
  switch (entry.status) {
    case "applied":
      return "codicon-check";
    case "failed":
      return "codicon-error";
    case "skipped":
      return "codicon-circle-slash";
    default:
      return null;
  }
}

/**
 * 批量卡片里的一行。有 diff 的行可以再点开看这一个文件的改动 —— 一次展开整批的
 * 全部 diff 会把卡片撑得没法读。
 */
function FileEntryRow({
  entry,
  onOpenFile,
}: {
  entry: WebviewToolDisplayFileEntry;
  onOpenFile: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const iconClass = fileEntryIconClass(entry);
  const hasDiff = (entry.diff?.length ?? 0) > 0;
  const hasStat =
    typeof entry.added === "number" || typeof entry.removed === "number";
  return (
    <li
      className="tc-tool-row__file"
      data-status={entry.status ?? "ok"}
      data-testid="tool-row-file-entry"
    >
      <div className="tc-tool-row__file-head">
        {iconClass ? (
          <span aria-hidden="true" className={`codicon ${iconClass}`} />
        ) : null}
        <FileChip onOpenFile={onOpenFile} path={entry.file} />
        {hasStat ? (
          <span className="tc-tool-row__diff-badges">
            <span className="tc-tool-row__diff-badge tc-tool-row__diff-badge--added">
              +{entry.added ?? 0}
            </span>
            <span className="tc-tool-row__diff-badge tc-tool-row__diff-badge--removed">
              -{entry.removed ?? 0}
            </span>
          </span>
        ) : null}
        {entry.range ? (
          <span
            className="tc-tool-row__file-range"
            data-testid="tool-row-file-range"
          >
            {entry.range}
          </span>
        ) : null}
        {entry.note ? (
          <span
            className="tc-tool-row__file-note"
            data-testid="tool-row-file-note"
          >
            {entry.note}
          </span>
        ) : null}
        {hasDiff ? (
          <button
            aria-expanded={expanded}
            aria-label={expanded ? "Collapse diff" : "Expand diff"}
            className="tc-tool-row__toggle"
            data-testid="tool-row-file-toggle"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            <span className="tc-tool-row__caret">{expanded ? "▾" : "▸"}</span>
          </button>
        ) : null}
      </div>
      {expanded && hasDiff ? (
        <DiffView
          diff={entry.diff ?? undefined}
          expired={entry.expired === true}
          truncated={entry.diffTruncated === true}
        />
      ) : null}
      {entry.expired === true ? (
        <div className="tc-diff-view__empty" data-testid="diff-view-expired">
          Diff 已超过 7 天保留期。
        </div>
      ) : null}
    </li>
  );
}

function renderFileEntries(
  entries: WebviewToolDisplayFileEntry[],
  onOpenFile: (path: string) => void,
): ReactNode {
  return (
    <ul className="tc-tool-row__files" data-testid="tool-row-files">
      {entries.map((entry, index) => (
        <FileEntryRow
          entry={entry}
          key={`${entry.file}-${index}`}
          onOpenFile={onOpenFile}
        />
      ))}
    </ul>
  );
}

/** `Edited 5 files` / `Read 3 files`：动词沿用单文件卡片，数量来自 display。 */
function buildFilesLabel(item: WebviewToolCard, count: number): string {
  const verb = buildFlatLabel(item).replace(/ files?$/, "");
  return `${verb} ${count} ${count === 1 ? "file" : "files"}`;
}

/**
 * `3 applied · 2 failed`。批量 edit 的部分成功必须在折叠态就能看见，否则
 * 「报了错是不是一个都没改」这个问题只能靠展开才能回答。
 */
function buildFilesStatusLabel(
  entries: WebviewToolDisplayFileEntry[],
): string | null {
  const counts = entries.reduce(
    (acc, entry) => {
      if (entry.status) {
        acc[entry.status] += 1;
      }
      return acc;
    },
    { applied: 0, failed: 0, skipped: 0 },
  );
  const parts: string[] = [];
  if (counts.applied > 0) {
    parts.push(`${counts.applied} applied`);
  }
  if (counts.failed > 0) {
    parts.push(`${counts.failed} failed`);
  }
  if (counts.skipped > 0) {
    parts.push(`${counts.skipped} skipped`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function sumFilesDiffStat(
  entries: WebviewToolDisplayFileEntry[],
): WebviewToolCard["diffStat"] | null {
  let added = 0;
  let removed = 0;
  let seen = false;
  for (const entry of entries) {
    if (typeof entry.added === "number" || typeof entry.removed === "number") {
      seen = true;
      added += entry.added ?? 0;
      removed += entry.removed ?? 0;
    }
  }
  return seen ? { added, removed } : null;
}

function renderDiffBadges(item: WebviewToolCard): ReactNode {
  if (!item.diffStat) {
    return null;
  }
  return (
    <span
      className="tc-tool-row__diff-badges"
      data-testid="tool-row-diff-badges"
    >
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
  const category = toolCategory(item.toolName);
  if (category === "answer") {
    return true;
  }
  if (category === "command") {
    return item.isError;
  }
  return item.isError || item.status !== "complete";
}

type ToolRowProps = {
  item: WebviewToolCard;
  onOpenFile(path: string): void;
  onOpenDiff?(toolCallId: string): void;
  onOpenPlanFile?(path: string): void;
  variant?: "grouped" | "standalone";
};

function ToolRowComponent({
  item,
  onOpenFile,
  onOpenDiff,
  onOpenPlanFile,
  variant = "standalone",
}: ToolRowProps) {
  const category = toolCategory(item.toolName);
  const filesDisplay = item.display?.kind === "files" ? item.display : null;
  const readFiles = (item.toolName === "read" || item.toolName === "read_file") ? filesDisplay : null;
  // A batch can succeed overall while individual files fail.
  const hasFailedFileEntry = Boolean(filesDisplay?.files.some((entry) => entry.status === "failed"));
  const terminalText =
    item.status === "complete" && !item.backgroundRunning
      ? item.summary
      : (item.liveOutput ?? item.summary);
  const boundedTerminalText = limitTerminalOutput(terminalText);
  const genericArgs = formatToolArgsForDisplay(item);
  const contentVisible = readFiles
    ? readFiles.files.length > 0
    : hasMeaningfulContent(item) || Boolean(boundedTerminalText) || Boolean(genericArgs);
  const alwaysVisibleBody = category === "answer" && contentVisible;
  const canToggle = contentVisible && !alwaysVisibleBody;
  const shouldExpandByDefault = shouldShowBodyByDefault(item, contentVisible) || Boolean(readFiles && hasFailedFileEntry);
  const [collapsed, setCollapsed] = useState(!shouldExpandByDefault);
  const [nowTick, setNowTick] = useState(() => Date.now());
  const [userInteracted, setUserInteracted] = useState(false);
  const countdownActive =
    isRunning(item) &&
    isBlockingTaskOutput(item) &&
    clampTaskOutputBudget(item.args?.wait_ms) > 0;

  useEffect(() => {
    setCollapsed(!shouldExpandByDefault);
    setUserInteracted(false);
  }, [item.id]);

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

  const iconClass = useMemo(
    () => toolIconClass(item.toolName),
    [item.toolName],
  );
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
  const fileDisplay = item.display?.kind === "file" ? item.display : undefined;
  const hasViewableDiff = isDiffViewable(item);
  const hasLargeDiffFallback =
    category === "edit" &&
    fileDisplay !== undefined &&
    !hasStructuredDiff &&
    Boolean(
      item.diffStat && (item.diffStat.added > 0 || item.diffStat.removed > 0),
    );
  const filesDiffStat = filesDisplay
    ? sumFilesDiffStat(filesDisplay.files)
    : null;
  const filesStatusLabel = filesDisplay
    ? buildFilesStatusLabel(filesDisplay.files)
    : null;
  const usesDisclosureCard =
    (category === "command" && contentVisible) ||
    (filesDisplay !== null && readFiles === null) ||
    (category === "edit" &&
      fileDisplay !== undefined &&
      (hasStructuredDiff || hasLargeDiffFallback));
  const disclosureStatusVariant: DisclosureStatusVariant = item.isError
    ? "error"
    : isRunningForDisplay(item)
      ? "running"
      : "success";
  const canOpenCurrentFile =
    category === "edit" &&
    fileDisplay !== undefined &&
    (hasStructuredDiff || hasLargeDiffFallback) &&
    Boolean(onOpenFile);
  const showOpenDiffButton =
    hasViewableDiff && Boolean(item.toolCallId) && Boolean(onOpenDiff);
  const showOpenFileButton = canOpenCurrentFile && !showOpenDiffButton;

  const disclosureHeader = (
    <div className="tc-tool-row__card-header">
      <span className="tc-tool-row__inline" data-testid="tool-row-label">
        {category === "command" ? (
          <>
            <span
              className={`tc-tool-row__text${loadingTextClass(isRunningForDisplay(item))}`}
              data-testid="tool-row-cmd-purpose"
            >
              {commandPurposeLabel(item)}
            </span>
            {commandBinaries(fullCommandText(item)).length > 0 ? (
              <span
                className="tc-tool-row__cmd-tags"
                data-testid="tool-row-cmd-tags"
              >
                {commandBinaries(fullCommandText(item)).join(", ")}
              </span>
            ) : null}
          </>
        ) : filesDisplay ? (
          <>
            <span className="tc-tool-row__text">
              {buildFilesLabel(item, filesDisplay.files.length)}
            </span>
            {filesDiffStat ? (
              <span
                className="tc-tool-row__diff-badges"
                data-testid="tool-row-diff-badges"
              >
                <span
                  className="tc-tool-row__diff-badge tc-tool-row__diff-badge--added"
                  data-testid="tool-row-diff-added"
                >
                  +{filesDiffStat.added}
                </span>
                <span
                  className="tc-tool-row__diff-badge tc-tool-row__diff-badge--removed"
                  data-testid="tool-row-diff-removed"
                >
                  -{filesDiffStat.removed}
                </span>
              </span>
            ) : null}
            {filesStatusLabel ? (
              <span
                className="tc-tool-row__files-status"
                data-testid="tool-row-files-status"
              >
                {filesStatusLabel}
              </span>
            ) : null}
          </>
        ) : (
          <>
            <span
              className={`tc-tool-row__text${loadingTextClass(isRunningForDisplay(item))}`}
            >
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
      {showOpenFileButton ? (
        <button
          aria-label="打开当前文件"
          className="tc-tool-row__action-link"
          data-testid="tool-row-open-file"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onOpenFile(fileDisplay!.file);
          }}
          type="button"
        >
          <span aria-hidden="true" className="codicon codicon-go-to-file" />
          <span>打开当前文件</span>
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
            defaultExpanded={shouldExpandByDefault || hasFailedFileEntry}
            header={disclosureHeader}
            leadingIcon={disclosureLeadingIcon}
            preview={
              filesDisplay ? null : category === "command" ? (
                <TerminalOutput
                  command={fullCommandText(item)}
                  preview
                  text={tailTerminalOutput(boundedTerminalText, 5)}
                />
              ) : (
                <DiffView
                  diff={item.diff}
                  expired={item.diffExpired === true}
                  previewRows={5}
                  truncated={item.diffTruncated === true}
                />
              )
            }
            resetKey={item.id}
            statusVariant={disclosureStatusVariant}
            toggleTestId="tool-row-toggle"
          >
            {filesDisplay ? (
              renderFileEntries(filesDisplay.files, onOpenFile)
            ) : category === "command" ? (
              <>
                <TerminalOutput
                  command={fullCommandText(item)}
                  text={boundedTerminalText}
                />
                {item.logPath ? (
                  <button
                    className="tc-tool-row__action-link"
                    data-testid="tool-row-full-log"
                    onClick={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      onOpenFile(item.logPath!);
                    }}
                    type="button"
                  >
                    Full log
                  </button>
                ) : null}
              </>
            ) : (
              <DiffView
                diff={item.diff}
                expired={item.diffExpired === true}
                truncated={item.diffTruncated === true}
              />
            )}
          </DisclosureCard>
        ) : (
          <>
            <div className="tc-tool-row__header">
              <span className="tc-tool-row__label" data-testid="tool-row-label">
                {readFiles ? (
                  <>
                    <span className={`tc-tool-row__text${loadingTextClass(isRunningForDisplay(item))}`}>
                      {buildFilesLabel(item, readFiles.files.length)}
                    </span>
                    {filesStatusLabel ? (
                      <span className="tc-tool-row__files-status" data-testid="tool-row-files-status">
                        {filesStatusLabel}
                      </span>
                    ) : null}
                  </>
                ) : renderFlatContent(item, onOpenFile, onOpenPlanFile, nowTick)}
              </span>
              {canToggle ? (
                <button
                  aria-expanded={!collapsed}
                  aria-label={
                    collapsed ? "Expand tool result" : "Collapse tool result"
                  }
                  className="tc-tool-row__toggle"
                  data-testid="tool-row-toggle"
                  onClick={() => {
                    setUserInteracted(true);
                    setCollapsed((value) => !value);
                  }}
                  type="button"
                >
                  <span className="tc-tool-row__caret">
                    {collapsed ? "▸" : "▾"}
                  </span>
                </button>
              ) : null}
            </div>
            {collapsed || !contentVisible ? null : (
              <div className="tc-tool-row__body" data-testid="tool-row-body">
                {readFiles ? renderFileEntries(readFiles.files, onOpenFile) : renderExpandedBody(item, genericArgs)}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

function areToolRowPropsEqual(prev: ToolRowProps, next: ToolRowProps): boolean {
  return (
    prev.item === next.item &&
    prev.onOpenDiff === next.onOpenDiff &&
    prev.onOpenFile === next.onOpenFile &&
    prev.onOpenPlanFile === next.onOpenPlanFile &&
    prev.variant === next.variant
  );
}

export const ToolRow = memo(ToolRowComponent, areToolRowPropsEqual);
ToolRow.displayName = "ToolRow";
