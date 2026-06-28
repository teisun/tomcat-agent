import { useEffect, useState, type ReactNode } from "react";

import type { AskQuestionResult, WebviewApprovalQuestion, WebviewToolCard } from "../types";
import { AnswerCard } from "./AnswerCard";
import { FileChip } from "./FileChip";

function firstLine(value: string | undefined): string | undefined {
  if (!value) {
    return undefined;
  }
  return value.split("\n").find((line) => line.trim())?.trim();
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function humanizeToolName(toolName: string): string {
  return toolName.replace(/_/g, " ");
}

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  return normalized.split("/").pop() || filePath;
}

function filePathForTool(item: WebviewToolCard): string | undefined {
  const args = item.args ?? {};
  return item.display?.kind === "file" ? item.display.file : asString(args.path);
}

export function isRunning(item: WebviewToolCard): boolean {
  return item.status !== "complete" && !item.isError;
}

function isReadLikeTool(toolName: string): boolean {
  return ["read", "read_file", "grep", "search_files"].includes(toolName);
}

function isEditLikeTool(toolName: string): boolean {
  return ["edit", "edit_file", "hashline_edit", "str_replace", "write", "write_file"].includes(
    toolName,
  );
}

function isCommandLikeTool(toolName: string): boolean {
  return ["bash", "shell", "execute_command"].includes(toolName);
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
    case "read":
    case "load_skill":
      return "codicon-book";
    case "grep":
    case "search_files":
    case "web_search":
    case "search_workspace":
      return "codicon-search";
    case "bash":
    case "task_output":
    case "task_list":
    case "task_stop":
      return "codicon-terminal";
    case "web_fetch":
      return "codicon-globe";
    case "edit":
    case "write":
    case "hashline_edit":
      return "codicon-edit";
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

  switch (item.toolName) {
    case "read":
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
      return running ? "Updating plan" : "Updated plan";
    }
    case "todos":
      return running ? "Updating todos" : "Updated todos";
    case "ask_question":
      return running ? "Asking question" : "Asked question";
    case "edit":
    case "write":
    case "hashline_edit":
      return running ? "Editing file" : "Edited file";
    default:
      return `${humanizeToolName(item.toolName)}${running ? "…" : ""}`;
  }
}

export function buildGroupTitleFromTool(item: WebviewToolCard): string {
  const filePath = filePathForTool(item);
  if (filePath && (item.toolName === "read" || item.toolName === "read_file")) {
    return `${buildFlatLabel(item)} ${basename(filePath)}`;
  }
  if (filePath && isEditLikeTool(item.toolName)) {
    return `${buildFlatLabel(item)} ${basename(filePath)}`;
  }
  return buildFlatLabel(item);
}

export function buildToolCollectionTitle(tools: WebviewToolCard[]): string {
  if (tools.length === 1) {
    return buildGroupTitleFromTool(tools[0]);
  }

  if (tools.every((tool) => isReadLikeTool(tool.toolName))) {
    return `Reviewed ${tools.length} files`;
  }
  if (tools.every((tool) => isEditLikeTool(tool.toolName))) {
    return `Edited ${tools.length} files`;
  }
  if (tools.every((tool) => isCommandLikeTool(tool.toolName))) {
    return tools.length === 1 ? buildGroupTitleFromTool(tools[0]) : `Executed ${tools.length} commands`;
  }

  return `Used ${tools.length} tools`;
}

export function hasMeaningfulContent(item: WebviewToolCard): boolean {
  if (item.summary?.trim()) {
    return true;
  }
  if (!item.display) {
    return false;
  }
  if (item.display.kind === "file") {
    return true;
  }
  if (item.display.kind === "plan") {
    return Boolean(item.display.plan.trim());
  }
  if (item.display.kind === "text") {
    return Boolean(item.display.text.trim());
  }
  return false;
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
    const parsed = JSON.parse(summary) as AskQuestionResult;
    if (!parsed || typeof parsed !== "object" || !Array.isArray(parsed.answers)) {
      return null;
    }
    if (typeof parsed.cancelled !== "boolean") {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

function renderFlatContent(
  item: WebviewToolCard,
  onOpenFile: (path: string) => void,
): ReactNode {
  const args = item.args ?? {};
  const filePath = filePathForTool(item);

  switch (item.toolName) {
    case "read":
    case "edit":
    case "write":
    case "hashline_edit":
      if (filePath) {
        return (
          <span className="tc-tool-row__inline">
            <span className="tc-tool-row__text">
              {buildFlatLabel(item).replace(/ file$/, "")}
            </span>
            <FileChip onOpenFile={onOpenFile} path={filePath} />
          </span>
        );
      }
      return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
    case "bash": {
      const command = firstLine(asString(args.command)) ?? "command";
      return (
        <span className="tc-tool-row__inline">
          <span className="tc-tool-row__text">{isRunning(item) ? "Running" : "Ran"}</span>
          <code className="tc-tool-row__cmd" data-testid="tool-row-cmd">
            {command}
          </code>
        </span>
      );
    }
    case "grep": {
      const resultsCount = countResults(item.summary);
      const suffix =
        !isRunning(item) && resultsCount ? ` · ${resultsCount} results` : "";
      const glob = asString(args.glob) ?? asString(args.path);
      if (glob) {
        return (
          <span className="tc-tool-row__inline">
            <span className="tc-tool-row__text">
              {buildFlatLabel(item)}
              {suffix}
            </span>
            <FileChip onOpenFile={onOpenFile} path={glob} />
          </span>
        );
      }
      return <span className="tc-tool-row__text">{`${buildFlatLabel(item)}${suffix}`}</span>;
    }
    case "search_workspace":
    case "web_search":
    case "web_fetch":
    default:
      return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
  }
}

function renderExpandedBody(item: WebviewToolCard): ReactNode {
  if (item.toolName === "ask_question") {
    const questions = parseApprovalQuestions(item.args);
    const result = parseAskQuestionResult(item.summary);
    if (questions && result) {
      return <AnswerCard questions={questions} result={result} />;
    }
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
      {item.summary ? <pre data-testid="tool-row-result">{item.summary}</pre> : null}
      {item.display?.kind === "plan" ? <pre>{item.display.plan}</pre> : null}
      {item.display?.kind === "text" && item.display.text !== item.summary ? (
        <pre>{item.display.text}</pre>
      ) : null}
    </>
  );
}

export function ToolRow({
  item,
  onOpenFile,
}: {
  item: WebviewToolCard;
  onOpenFile(path: string): void;
}) {
  const contentVisible = hasMeaningfulContent(item);
  const shouldExpandByDefault = item.isError || (item.status !== "complete" && contentVisible);
  const [collapsed, setCollapsed] = useState(!shouldExpandByDefault);
  const [userInteracted, setUserInteracted] = useState(false);

  useEffect(() => {
    setCollapsed(!shouldExpandByDefault);
    setUserInteracted(false);
  }, [item.id, shouldExpandByDefault]);

  useEffect(() => {
    if (!userInteracted) {
      setCollapsed(!shouldExpandByDefault);
    }
  }, [shouldExpandByDefault, userInteracted]);

  return (
    <div className="tc-thinking-tool-wrapper" data-testid="tool-row-wrapper">
      <span
        aria-hidden="true"
        className={`tc-thinking-icon codicon ${toolIconClass(item.toolName)}`}
      />
      <div className="tc-tool-row" data-testid="tool-row">
        <div className="tc-tool-row__header">
          <span className="tc-tool-row__label" data-testid="tool-row-label">
            {renderFlatContent(item, onOpenFile)}
            {isRunning(item) ? (
              <span
                aria-hidden="true"
                className="tc-thinking__dots tc-tool-row__running"
                data-testid="tool-row-running-indicator"
              >
                ...
              </span>
            ) : null}
          </span>
          {contentVisible ? (
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
      </div>
    </div>
  );
}
