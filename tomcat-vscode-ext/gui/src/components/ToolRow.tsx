import { useEffect, useMemo, useState, type ReactNode } from "react";

import type { AskQuestionResult, WebviewApprovalQuestion, WebviewToolCard } from "../types";
import { AnswerCard } from "./AnswerCard";
import { DiffView } from "./DiffView";
import { DisclosureCard, type DisclosureStatusVariant } from "./DisclosureCard";
import { FileChip } from "./FileChip";
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

function humanizeToolName(toolName: string): string {
  return toolName.replace(/_/g, " ");
}

export type ToolCategory = "answer" | "command" | "context" | "edit" | "other";

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

export function isRunning(item: WebviewToolCard): boolean {
  return (item.status === "running" || item.status === "streaming") && !item.isError;
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
  const category = toolCategory(item.toolName);
  return category === "answer" || category === "command" || category === "edit";
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

export function hasMeaningfulContent(item: WebviewToolCard): boolean {
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
): ReactNode {
  const args = item.args ?? {};
  const filePath = filePathForTool(item);
  const category = toolCategory(item.toolName);
  const diffStat = item.diffStat;

  switch (category) {
    case "edit":
      if (filePath) {
        return (
          <span className="tc-tool-row__inline">
            <span className="tc-tool-row__text">{buildFlatLabel(item).replace(/ file$/, "")}</span>
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
      return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
    case "command": {
      const command =
        firstLine(asString(args.command)) ??
        firstLine(asString(args.cmd)) ??
        firstLine(asString(args.script)) ??
        "command";
      const verb =
        item.status === "interrupted"
          ? "Interrupted"
          : isRunning(item)
            ? "Running"
            : "Ran";
      return (
        <span className="tc-tool-row__inline">
          <span className="tc-tool-row__text">{verb}</span>
          <code className="tc-tool-row__cmd" data-testid="tool-row-cmd">
            {command}
          </code>
        </span>
      );
    }
    case "answer":
      return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
    case "context":
    case "other":
      switch (item.toolName) {
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
        case "read":
        case "read_file":
          if (filePath) {
            return (
              <span className="tc-tool-row__inline">
                <span className="tc-tool-row__text">{buildFlatLabel(item).replace(/ file$/, "")}</span>
                <FileChip onOpenFile={onOpenFile} path={filePath} />
              </span>
            );
          }
          return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
        default:
          return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
      }
    default:
      return <span className="tc-tool-row__text">{buildFlatLabel(item)}</span>;
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
  item,
  onOpenFile,
  onOpenDiff,
  variant = "standalone",
}: {
  item: WebviewToolCard;
  onOpenFile(path: string): void;
  onOpenDiff?(toolCallId: string): void;
  variant?: "grouped" | "standalone";
}) {
  const category = toolCategory(item.toolName);
  const contentVisible = hasMeaningfulContent(item);
  const alwaysVisibleBody = category === "answer" && contentVisible;
  const canToggle = contentVisible && !alwaysVisibleBody;
  const shouldExpandByDefault = shouldShowBodyByDefault(item, contentVisible);
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

  const iconClass = useMemo(() => toolIconClass(item.toolName), [item.toolName]);
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

  const labelWithRunningIndicator = (label: ReactNode) => (
    <>
      {label}
      {isRunning(item) ? (
        <span
          aria-hidden="true"
          className="tc-thinking__dots tc-tool-row__running"
          data-testid="tool-row-running-indicator"
        >
          ...
        </span>
      ) : null}
    </>
  );

  const disclosureHeader =
    category === "command" ? (
      <span className="tc-tool-row__inline" data-testid="tool-row-label">
        {labelWithRunningIndicator(
          <>
            <span className="tc-tool-row__text">
              {item.status === "interrupted" ? "Interrupted" : isRunning(item) ? "Running" : "Ran"}
            </span>
            <code className="tc-tool-row__cmd" data-testid="tool-row-cmd">
              {commandText(item)}
            </code>
          </>,
        )}
      </span>
    ) : (
      <div className="tc-tool-row__card-header">
        <span className="tc-tool-row__inline" data-testid="tool-row-label">
          {labelWithRunningIndicator(
            <>
              <span className="tc-tool-row__text">{buildFlatLabel(item).replace(/ file$/, "")}</span>
              {item.display?.kind === "file" ? (
                <FileChip onOpenFile={onOpenFile} path={item.display.file} />
              ) : null}
              {renderDiffBadges(item)}
            </>,
          )}
        </span>
        {showOpenDiffButton ? (
          <button
            aria-label="View diff"
            className="tc-tool-row__action-icon"
            data-testid="tool-row-open-diff"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onOpenDiff?.(item.toolCallId);
            }}
            type="button"
          >
            <span aria-hidden="true" className="codicon codicon-diff" />
          </button>
        ) : null}
      </div>
    );

  return (
    <div className={shellClassName} data-testid="tool-row-wrapper">
      {iconNode}
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
            preview={
              category === "command" ? (
                <TerminalOutput preview text={tailTerminalOutput(item.summary, 5)} />
              ) : (
                <DiffView diff={item.diff} previewLines={5} />
              )
            }
            resetKey={item.id}
            statusVariant={disclosureStatusVariant}
            toggleTestId="tool-row-toggle"
          >
            {category === "command" ? (
              <TerminalOutput text={item.summary} />
            ) : (
              <DiffView diff={item.diff} />
            )}
          </DisclosureCard>
        ) : (
          <>
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
