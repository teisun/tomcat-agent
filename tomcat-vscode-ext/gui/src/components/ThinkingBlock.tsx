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
}: {
  isStreaming?: boolean;
  item: WebviewThinkingBlock;
}) {
  const [collapsed, setCollapsed] = useState(true);
  const summary = useMemo(() => summarizeThinking(item.text), [item.text]);

  useEffect(() => {
    setCollapsed(true);
  }, [item.id]);

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
        <span className="tc-thinking__heading">
          <span className="tc-thinking__title">
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
          {collapsed && isStreaming && summary ? (
            <span className="tc-thinking__summary" data-testid="thinking-summary">
              {summary}
            </span>
          ) : null}
        </span>
        <span>{collapsed ? "▸" : "▾"}</span>
      </button>
      {collapsed ? null : <pre>{item.text}</pre>}
    </section>
  );
}
