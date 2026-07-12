import { useEffect, useMemo, useState } from "react";

import type { WebviewMessageBlock, WebviewToolCard } from "../types";
import { MessageBubble } from "./MessageBubble";
import { ThinkingBlock } from "./ThinkingBlock";
import { buildToolCollectionTitle, isSuppressedPlanToolRow, ToolRow } from "./ToolRow";
import type { AssistantResponseGroup } from "./sessionList/groupTimelineByAssistantResponse";

function isDirtySummaryTitle(summaryTitle: string, tools: WebviewToolCard[]): boolean {
  if (/[{\[]/.test(summaryTitle) || /\b(path|command)=/.test(summaryTitle)) {
    return true;
  }
  const normalized = summaryTitle.trim().toLowerCase();
  return tools.some((tool) => normalized.startsWith(tool.toolName.toLowerCase()));
}

function groupHeaderTitle(
  group: AssistantResponseGroup,
  isStreaming: boolean,
): { shimmer: boolean; text: string } {
  const summaryTitle = group.thinking?.summaryTitle ?? null;
  if (summaryTitle && group.tools.length > 0 && !isDirtySummaryTitle(summaryTitle, group.tools)) {
    return { shimmer: false, text: summaryTitle };
  }
  if (group.tools.length > 0) {
    return { shimmer: isStreaming, text: buildToolCollectionTitle(group.tools) };
  }
  return { shimmer: isStreaming, text: "Thinking" };
}

function shouldHideGroupedToolRow(tool: WebviewToolCard): boolean {
  return isSuppressedPlanToolRow(tool);
}

export function ThinkingGroup({
  group,
  isStreaming = false,
  onOpenFile,
  onOpenDiff,
}: {
  group: AssistantResponseGroup;
  isStreaming?: boolean;
  onOpenFile(path: string): void;
  onOpenDiff?(toolCallId: string): void;
}) {
  const streaming = isStreaming && group.tools.some((tool) => tool.status !== "complete");
  const [collapsed, setCollapsed] = useState(!streaming);

  useEffect(() => {
    setCollapsed(!streaming);
  }, [group.assistantMessageId, streaming]);

  const header = useMemo(() => groupHeaderTitle(group, isStreaming), [group, isStreaming]);
  const statusIconClass = isStreaming
    ? "tc-thinking-box__status codicon codicon-loading tc-codicon-spin"
    : group.tools.length > 0
      ? "tc-thinking-box__status codicon codicon-search"
      : "tc-thinking-box__status codicon codicon-lightbulb";

  const preamble = group.preamble;
  const thinking = group.thinking;
  const tools = group.tools.filter((tool) => !shouldHideGroupedToolRow(tool));

  return (
    <section
      className="tc-thinking-box"
      data-assistant-message-id={group.assistantMessageId}
      data-testid="thinking-group"
    >
      {preamble ? (
        <MessageBubble item={preamble as WebviewMessageBlock} />
      ) : null}
      <div className="tc-thinking-list">
        <button
          aria-expanded={!collapsed}
          className="tc-thinking-box__header"
          data-testid="thinking-group-toggle"
          onClick={() => setCollapsed((value) => !value)}
          type="button"
        >
          <span className="tc-thinking-box__lead">
            <span
              aria-hidden="true"
              className={statusIconClass}
              data-testid="thinking-group-status"
            />
            <span
              className={`tc-thinking__title${header.shimmer ? " tc-thinking__title--shimmer" : ""}`}
              data-testid="thinking-group-title"
            >
              {header.text}
            </span>
          </span>
          <span className="tc-thinking-box__caret">{collapsed ? "▸" : "▾"}</span>
        </button>
        {collapsed ? null : (
          <>
            {thinking ? <ThinkingBlock item={thinking} variant="embedded" /> : null}
            {tools.map((tool: WebviewToolCard) => (
              <ToolRow
                item={tool}
                key={tool.id}
                onOpenDiff={onOpenDiff}
                onOpenFile={onOpenFile}
                variant="grouped"
              />
            ))}
          </>
        )}
      </div>
    </section>
  );
}
