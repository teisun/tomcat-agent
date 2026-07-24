import { memo, useEffect, useMemo, useState } from "react";

import type { WebviewThinkingBlock } from "../types";

function normalizeThinkingDisplayText(text: string): string {
  return text
    .split("\n")
    .map((line) => {
      const trimmed = line.trim();
      if (!/^(?:\*\*.+?\*\*\s*){2,}$/.test(trimmed)) {
        return line;
      }

      const segments = trimmed.match(/\*\*.+?\*\*/g);
      return segments ? segments.join("\n") : line;
    })
    .join("\n");
}

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

function ThinkingBlockComponent({
  isStreaming = false,
  item,
  variant = "standalone",
}: {
  isStreaming?: boolean;
  item: WebviewThinkingBlock;
  onOpenFile?: (path: string, line?: number) => void;
  variant?: "embedded" | "standalone";
}) {
  const [collapsed, setCollapsed] = useState(true);
  const displayText = useMemo(() => normalizeThinkingDisplayText(item.text), [item.text]);
  const summary = useMemo(() => summarizeThinking(displayText), [displayText]);

  useEffect(() => {
    setCollapsed(true);
  }, [item.id]);

  if (variant === "embedded") {
    return displayText ? (
      <pre className="tc-thinking-box__body" data-testid="thinking-group-body">
        {displayText}
      </pre>
    ) : null;
  }

  const statusIconClass = "tc-thinking__status codicon codicon-lightbulb";

  return (
    <section
      className={`tc-thinking${isStreaming ? " tc-thinking--streaming" : ""}`}
      data-testid="thinking-block"
    >
      <button
        aria-label={collapsed ? "Expand thinking" : "Collapse thinking"}
        className="tc-thinking__toggle"
        data-testid="thinking-toggle"
        onClick={() => setCollapsed((value) => !value)}
        type="button"
      >
        <span className="tc-thinking__lead">
          <span
            aria-hidden="true"
            className={statusIconClass}
            data-testid="thinking-status"
          />
          <span className="tc-thinking__heading">
            <span
              className={`tc-thinking__title${isStreaming ? " tc-thinking__title--shimmer tc-loading-shimmer" : ""}`}
            >
              <span>Thinking</span>
            </span>
            {collapsed && summary ? (
              <span className="tc-thinking__summary" data-testid="thinking-summary">
                {summary}
              </span>
            ) : null}
          </span>
        </span>
        <span className="tc-thinking__caret">{collapsed ? "▸" : "▾"}</span>
      </button>
      {collapsed ? null : (
        <pre className="tc-thinking__body" data-testid="thinking-body">
          {displayText}
        </pre>
      )}
    </section>
  );
}

function areThinkingBlockPropsEqual(
  previous: Readonly<Parameters<typeof ThinkingBlockComponent>[0]>,
  next: Readonly<Parameters<typeof ThinkingBlockComponent>[0]>,
): boolean {
  return (
    previous.isStreaming === next.isStreaming &&
    previous.item === next.item &&
    previous.variant === next.variant
  );
}

export const ThinkingBlock = memo(ThinkingBlockComponent, areThinkingBlockPropsEqual);
