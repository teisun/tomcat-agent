import { useEffect, useState } from "react";

import type { WebviewToolCard } from "../types";

function toolStatusLabel(item: WebviewToolCard): string {
  if (item.isError) {
    return "Error";
  }
  return item.status === "complete" ? "Done" : "Running";
}

function toolStatusIcon(item: WebviewToolCard): string {
  if (item.isError) {
    return "✕";
  }
  return item.status === "complete" ? "✓" : "◌";
}

export function ToolCallCard({
  item,
  onApplyEdit,
  onOpenDiff,
}: {
  item: WebviewToolCard;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
}) {
  const shouldExpandByDefault = item.isError || item.status !== "complete";
  const [collapsed, setCollapsed] = useState(!shouldExpandByDefault);
  const [userInteracted, setUserInteracted] = useState(false);
  const toolLabel = toolStatusLabel(item);
  const textDisplay = item.display?.kind === "text" ? item.display.text : null;
  const showTextDisplay = Boolean(textDisplay && textDisplay !== item.summary);

  useEffect(() => {
    setCollapsed(!shouldExpandByDefault);
    setUserInteracted(false);
  }, [item.id]);

  useEffect(() => {
    if (!userInteracted) {
      setCollapsed(!shouldExpandByDefault);
    }
  }, [shouldExpandByDefault, userInteracted]);

  return (
    <section className="tc-card" data-testid="tool-card">
      <button
        aria-expanded={!collapsed}
        className="tc-tool-card__toggle"
        data-testid="tool-toggle"
        onClick={() => {
          setUserInteracted(true);
          setCollapsed((value) => !value);
        }}
        type="button"
      >
        <span className="tc-tool-card__summary">
          <span
            aria-hidden="true"
            className={`tc-tool-card__status-icon${item.status !== "complete" && !item.isError ? " tc-tool-card__status-icon--running" : ""}`}
          >
            {toolStatusIcon(item)}
          </span>
          <h3 className="tc-tool-card__title" data-testid="tool-title">
            {item.toolName} ({item.status})
          </h3>
        </span>
        <span className={item.isError ? "tc-chip tc-chip--danger" : "tc-chip tc-chip--success"}>
          {toolLabel}
        </span>
        <span className="tc-tool-card__caret">{collapsed ? "▸" : "▾"}</span>
      </button>
      {collapsed ? null : (
        <div className="tc-tool-card__body" data-testid="tool-body">
          {item.summary ? <pre>{item.summary}</pre> : null}
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
          {showTextDisplay ? <pre>{textDisplay}</pre> : null}
        </div>
      )}
    </section>
  );
}
