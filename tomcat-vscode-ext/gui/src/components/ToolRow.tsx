import { useEffect, useState, type ReactNode } from "react";

import type { WebviewToolCard } from "../types";
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

function isRunning(item: WebviewToolCard): boolean {
  return item.status !== "complete" && !item.isError;
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

function buildFlatLabel(item: WebviewToolCard): string {
  const args = item.args ?? {};
  const running = isRunning(item);

  switch (item.toolName) {
    case "read":
      return running ? "Reading file" : "Read";
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
      const goal = asString(args.goal) ?? "plan";
      return running ? `Creating plan ${goal}` : `Created plan ${goal}`;
    }
    case "update_plan": {
      const planId = asString(args.plan_id) ?? asString(args.planId) ?? "plan";
      return running ? `Updating plan ${planId}` : `Updated plan ${planId}`;
    }
    case "todos":
      return running ? "Updating todos" : "Updated todos";
    case "ask_question":
      return running ? "Asking question" : "Asked question";
    case "edit":
    case "write":
    case "hashline_edit":
      return running ? "Editing file" : "Edited";
    default:
      return `${humanizeToolName(item.toolName)}${running ? "…" : ""}`;
  }
}

function renderFlatContent(
  item: WebviewToolCard,
  onOpenFile: (path: string) => void,
): ReactNode {
  const args = item.args ?? {};
  const filePath =
    item.display?.kind === "file"
      ? item.display.file
      : asString(args.path);

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

function renderExpandedBody(
  item: WebviewToolCard,
  onApplyEdit: (toolCallId: string) => void,
  onOpenDiff: (toolCallId: string) => void,
): ReactNode {
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
      {item.display?.kind === "file" ? (
        <div className="tc-button-row">
          <button
            className="tc-button tc-button--secondary"
            onClick={() => onOpenDiff(item.toolCallId)}
            type="button"
          >
            Open Diff
          </button>
          <button
            className="tc-button tc-button--primary"
            onClick={() => onApplyEdit(item.toolCallId)}
            type="button"
          >
            Apply Edit
          </button>
        </div>
      ) : null}
      {item.display?.kind === "plan" ? <pre>{item.display.plan}</pre> : null}
      {item.display?.kind === "text" && item.display.text !== item.summary ? (
        <pre>{item.display.text}</pre>
      ) : null}
    </>
  );
}

export function ToolRow({
  item,
  onApplyEdit,
  onOpenDiff,
  onOpenFile,
}: {
  item: WebviewToolCard;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
  onOpenFile(path: string): void;
}) {
  const shouldExpandByDefault = item.isError || item.status !== "complete";
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
          </span>
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
        </div>
        {collapsed ? null : (
          <div className="tc-tool-row__body" data-testid="tool-row-body">
            {renderExpandedBody(item, onApplyEdit, onOpenDiff)}
          </div>
        )}
      </div>
    </div>
  );
}
