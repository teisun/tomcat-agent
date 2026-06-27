import type { WebviewPlanFileCard, WebviewTodo } from "../types";

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || filePath;
}

function todoCountLabel(count: number): string {
  return `${count} ${count === 1 ? "todo" : "todos"}`;
}

export function PlanFileCard({
  canBuild,
  item,
  onBuild,
  onOpenPlanFile,
  planTodos = [],
}: {
  canBuild: boolean;
  item: WebviewPlanFileCard;
  onBuild(): void;
  onOpenPlanFile(path: string): void;
  planTodos?: WebviewTodo[];
}) {
  const fileName = basename(item.path);
  const title = item.title?.trim() || fileName;
  const buildAllowed =
    canBuild && (item.state === "planning" || item.state === "pending");

  return (
    <section className="tc-card tc-plan-card" data-testid="plan-card">
      <div className="tc-plan-card__file-row">
        <span aria-hidden="true" className="tc-plan-card__file-icon codicon codicon-list-tree" />
        <span className="tc-plan-card__file-name" data-testid="plan-card-file-name">
          {fileName}
        </span>
      </div>
      <button
        aria-label="Open plan file"
        className="tc-plan-card__title"
        data-testid="plan-card-title"
        onClick={() => onOpenPlanFile(item.path)}
        type="button"
      >
        {title}
      </button>
      {item.overview ? (
        <p className="tc-plan-card__overview" data-testid="plan-card-overview">
          {item.overview}
        </p>
      ) : null}
      <div className="tc-plan-card__todos-count" data-testid="plan-todos-count">
        {todoCountLabel(planTodos.length)}
      </div>
      <div className="tc-plan-card__footer">
        <button
          aria-label="View plan file"
          className="tc-plan-card__footer-link"
          data-testid="view-plan"
          onClick={() => onOpenPlanFile(item.path)}
          type="button"
        >
          View Plan
        </button>
        <button
          className="tc-button tc-button--primary"
          data-testid="build-plan"
          disabled={!buildAllowed}
          onClick={onBuild}
          type="button"
        >
          Build
        </button>
      </div>
    </section>
  );
}
