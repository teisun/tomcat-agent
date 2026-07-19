import { PlanBuildModelSelect } from "./PlanBuildModelSelect";
import type { WebviewPlanFileCard, WebviewTodo } from "../types";

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || filePath;
}

function prettifyPlanToken(value: string): string {
  return value
    .replace(/^plan_/, "")
    .replace(/_[0-9a-f]{8}$/i, "")
    .replace(/_/g, " ")
    .trim();
}

function derivePlanTitle(item: WebviewPlanFileCard, fileName: string): string {
  const explicitTitle = item.title?.trim();
  if (explicitTitle && explicitTitle !== fileName) {
    return explicitTitle;
  }

  const overviewTitle = item.overview?.trim().split("\n")[0]?.trim();
  if (overviewTitle) {
    return overviewTitle.length > 96 ? `${overviewTitle.slice(0, 93).trimEnd()}...` : overviewTitle;
  }

  const prettyPlanId = item.planId ? prettifyPlanToken(item.planId) : "";
  if (prettyPlanId) {
    return prettyPlanId;
  }

  return fileName;
}

function todoCountLabel(count: number): string {
  return `${count} ${count === 1 ? "todo" : "todos"}`;
}

export function PlanFileCard({
  availableModels = [],
  buildModel = "",
  canBuild,
  creating = false,
  item,
  onBuild,
  onOpenPlanFile,
  onSetBuildModel,
  planTodos = [],
}: {
  availableModels?: string[];
  buildModel?: string;
  canBuild: boolean;
  creating?: boolean;
  item: WebviewPlanFileCard;
  onBuild(planId: string | null, path: string): void;
  onOpenPlanFile(path: string): void;
  onSetBuildModel?(modelId: string): void;
  planTodos?: WebviewTodo[];
}) {
  const fileName = basename(item.path);
  const title = derivePlanTitle(item, fileName);
  const buildAllowed =
    canBuild && (item.state === "planning" || item.state === "pending");

  return (
    <section className="tc-card tc-plan-card" data-testid="plan-card">
      <button
        aria-label="Open plan file"
        className="tc-plan-card__file-row"
        data-testid="plan-card-file-link"
        onClick={() => onOpenPlanFile(item.path)}
        type="button"
      >
        <span aria-hidden="true" className="tc-plan-card__file-icon codicon codicon-list-tree" />
        <span className="tc-plan-card__file-name" data-testid="plan-card-file-name">
          {fileName}
        </span>
      </button>
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
        {todoCountLabel(item.todos?.length ?? planTodos.length)}
      </div>
      <div className="tc-plan-card__footer">
        {creating ? (
          <button
            aria-busy="true"
            aria-label="Creating plan file"
            className="tc-plan-card__footer-link tc-plan-card__footer-link--busy"
            data-testid="view-plan-pending"
            disabled
            type="button"
          >
            <span aria-hidden="true" className="tc-thinking__dots tc-plan-card__footer-dots">
              ...
            </span>
          </button>
        ) : (
          <button
            aria-label="View plan file"
            className="tc-plan-card__footer-link"
            data-testid="view-plan"
            onClick={() => onOpenPlanFile(item.path)}
            type="button"
          >
            View Plan
          </button>
        )}
        {onSetBuildModel ? (
          <PlanBuildModelSelect
            availableModels={availableModels}
            label="Model"
            onChange={onSetBuildModel}
            testId="plan-card-build-model"
            value={buildModel}
          />
        ) : null}
        <button
          className="tc-button tc-plan-build-button"
          data-testid="build-plan"
          disabled={!buildAllowed}
          onClick={() => onBuild(item.planId ?? null, item.path)}
          type="button"
        >
          Build
        </button>
      </div>
    </section>
  );
}
