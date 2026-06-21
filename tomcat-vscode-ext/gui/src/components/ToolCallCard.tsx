import type { WebviewToolCard } from "../types";

export function ToolCallCard({
  item,
  onApplyEdit,
  onOpenDiff,
}: {
  item: WebviewToolCard;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
}) {
  return (
    <section className="tc-card" data-testid="tool-card">
      <div className="tc-card__header">
        <h3 data-testid="tool-title">
          {item.toolName} ({item.status})
        </h3>
        <span className={item.isError ? "tc-chip tc-chip--danger" : "tc-chip tc-chip--success"}>
          {item.isError ? "Error" : item.status === "complete" ? "Done" : "Running"}
        </span>
      </div>
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
      {item.display?.kind === "text" ? <pre>{item.display.text}</pre> : null}
    </section>
  );
}
