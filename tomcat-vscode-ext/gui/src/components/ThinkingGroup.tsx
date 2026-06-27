import { useEffect, useMemo, useState } from "react";

import type { WebviewMessageBlock, WebviewToolCard } from "../types";
import { MessageBubble } from "./MessageBubble";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolRow } from "./ToolRow";
import type { AssistantResponseGroup } from "./sessionList/groupTimelineByAssistantResponse";

function summarizeThinking(text: string): string | null {
  const firstMeaningfulLine = text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean);
  if (!firstMeaningfulLine) {
    return null;
  }
  return firstMeaningfulLine.length > 140
    ? `${firstMeaningfulLine.slice(0, 137).trimEnd()}...`
    : firstMeaningfulLine;
}

function groupHeaderTitle(
  group: AssistantResponseGroup,
  isStreaming: boolean,
): { shimmer: boolean; text: string } {
  const summaryTitle = group.thinking?.summaryTitle ?? null;
  if (summaryTitle) {
    return { shimmer: false, text: summaryTitle };
  }
  if (isStreaming) {
    const fallback = summarizeThinking(group.thinking?.text ?? "") ?? "Tomcat · Thinking";
    return { shimmer: true, text: fallback };
  }
  const fallback = summarizeThinking(group.thinking?.text ?? "");
  if (fallback) {
    return { shimmer: false, text: `Tomcat · Thinking — ${fallback}` };
  }
  return { shimmer: false, text: "Tomcat · Thinking" };
}

export function ThinkingGroup({
  group,
  isStreaming = false,
  onApplyEdit,
  onOpenDiff,
  onOpenFile,
}: {
  group: AssistantResponseGroup;
  isStreaming?: boolean;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
  onOpenFile(path: string): void;
}) {
  const streaming = isStreaming && group.tools.some((tool) => tool.status !== "complete");
  const [collapsed, setCollapsed] = useState(!streaming);

  useEffect(() => {
    setCollapsed(!streaming);
  }, [group.assistantMessageId, streaming]);

  const header = useMemo(() => groupHeaderTitle(group, isStreaming), [group, isStreaming]);
  const statusIconClass = isStreaming
    ? "tc-thinking-box__status codicon codicon-loading tc-codicon-spin"
    : "tc-thinking-box__status codicon codicon-check";

  const preamble = group.preamble;
  const thinking = group.thinking;
  const tools = group.tools;

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
                onApplyEdit={onApplyEdit}
                onOpenDiff={onOpenDiff}
                onOpenFile={onOpenFile}
              />
            ))}
          </>
        )}
      </div>
    </section>
  );
}
