import { useEffect, useMemo, useState } from "react";

import type { WebviewThinkingBlock } from "../types";

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

export function ThinkingBlock({
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
  const summary = useMemo(() => summarizeThinking(item.text), [item.text]);

  useEffect(() => {
    setCollapsed(true);
  }, [item.id]);

  if (variant === "embedded") {
    return item.text ? (
      <pre className="tc-thinking-box__body" data-testid="thinking-group-body">
        {item.text}
      </pre>
    ) : null;
  }

  const statusIconClass = isStreaming
    ? "tc-thinking__status codicon codicon-loading tc-codicon-spin"
    : "tc-thinking__status codicon codicon-check";

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
              className={`tc-thinking__title${isStreaming ? " tc-thinking__title--shimmer" : ""}`}
            >
              <span>Tomcat · Thinking</span>
              {isStreaming ? (
                <span
                  aria-hidden="true"
                  className="tc-thinking__dots"
                  data-testid="thinking-streaming-indicator"
                >
                  ...
                </span>
              ) : null}
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
          {item.text}
        </pre>
      )}
    </section>
  );
}
