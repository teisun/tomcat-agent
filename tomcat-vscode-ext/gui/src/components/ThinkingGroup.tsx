import { memo, useEffect, useMemo, useState } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import type { Locale, Translator } from "../../../src/shared/i18n";

import type { WebviewMediaRoot, WebviewMessageBlock, WebviewToolCard } from "../types";
import { GroupActivityTicker } from "./GroupActivityTicker";
import { MessageBubble } from "./MessageBubble";
import { ThinkingBlock } from "./ThinkingBlock";
import { buildToolCollectionTitle, ToolRow } from "./ToolRow";
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
  t: Translator,
  locale: Locale,
): { shimmer: boolean; text: string } {
  const summaryTitle = group.thinking?.summaryTitle ?? null;
  if (summaryTitle && (group.tools.length === 0 || !isDirtySummaryTitle(summaryTitle, group.tools))) {
    return { shimmer: isStreaming, text: summaryTitle };
  }
  if (group.tools.length > 0) {
    return { shimmer: isStreaming, text: buildToolCollectionTitle(group.tools, t, locale) };
  }
  return { shimmer: isStreaming, text: t("thinking.title") };
}

type ThinkingGroupProps = {
  group: AssistantResponseGroup;
  isLive?: boolean;
  isStreaming?: boolean;
  mediaRoots?: WebviewMediaRoot[];
  onOpenFile(path: string, line?: number): void;
  onOpenDiff?(toolCallId: string): void;
  onOpenImagePreview?(attachmentId: string): void;
  onZoomImage?(image: { alt: string; src: string }): void;
};

function ThinkingGroupComponent({
  group,
  isLive = false,
  isStreaming = false,
  mediaRoots,
  onOpenFile,
  onOpenDiff,
  onOpenImagePreview,
  onZoomImage,
}: ThinkingGroupProps) {
  const t = useT();
  const locale = useLocale();
  const streaming = isStreaming && group.tools.some((tool) => tool.status !== "complete");
  const [collapsed, setCollapsed] = useState(true);

  useEffect(() => {
    setCollapsed(true);
  }, [group.assistantMessageId]);

  const header = useMemo(() => groupHeaderTitle(group, isStreaming, t, locale), [group, isStreaming, t, locale]);
  const statusIconClass =
    group.tools.length > 0
      ? "tc-thinking-box__status codicon codicon-search"
      : "tc-thinking-box__status codicon codicon-lightbulb";

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
        <MessageBubble
          item={preamble as WebviewMessageBlock}
          mediaRoots={mediaRoots}
          onOpenFile={onOpenFile}
          onZoomImage={onZoomImage}
        />
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
              className={`tc-thinking__title tc-thinking-box__title${header.shimmer ? " tc-thinking__title--shimmer tc-loading-shimmer" : ""}`}
              data-testid="thinking-group-title"
              title={header.text}
            >
              {header.text}
            </span>
          </span>
          <span className="tc-thinking-box__caret">{collapsed ? "▸" : "▾"}</span>
        </button>
        {collapsed && tools.length > 0 ? (
          <GroupActivityTicker isLive={isLive} tools={tools} />
        ) : null}
        {collapsed ? null : (
          <>
            {thinking ? <ThinkingBlock item={thinking} onOpenFile={onOpenFile} variant="embedded" /> : null}
            {tools.map((tool: WebviewToolCard) => (
              <ToolRow
                item={tool}
                key={tool.id}
                onOpenDiff={onOpenDiff}
                onOpenImagePreview={onOpenImagePreview}
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

function sameTools(left: WebviewToolCard[], right: WebviewToolCard[]): boolean {
  return left.length === right.length && left.every((tool, index) => tool === right[index]);
}

function areThinkingGroupPropsEqual(prev: ThinkingGroupProps, next: ThinkingGroupProps): boolean {
  return (
    prev.group.assistantMessageId === next.group.assistantMessageId &&
    prev.group.preamble === next.group.preamble &&
    prev.group.thinking === next.group.thinking &&
    sameTools(prev.group.tools, next.group.tools) &&
    prev.isLive === next.isLive &&
    prev.isStreaming === next.isStreaming &&
    prev.mediaRoots === next.mediaRoots &&
    prev.onOpenDiff === next.onOpenDiff &&
    prev.onOpenImagePreview === next.onOpenImagePreview &&
    prev.onOpenFile === next.onOpenFile &&
    prev.onZoomImage === next.onZoomImage
  );
}

export const ThinkingGroup = memo(ThinkingGroupComponent, areThinkingGroupPropsEqual);
ThinkingGroup.displayName = "ThinkingGroup";
