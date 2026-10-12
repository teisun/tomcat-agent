import { memo } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, t as defaultT, type Translator } from "../../../src/shared/i18n";

import type { Speed } from "../../../src/shared/modelSpeed";
import { buildPickerModels } from "./buildPickerModels";
import { ModelPicker, type ModelPickerModel } from "./ModelPicker";
import type {
  WebviewPlanFileCard,
  WebviewPlanFileState,
  WebviewTodo,
  WebviewToolCard,
} from "../types";
import { LoadingDots } from "./LoadingDots";

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

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function planPathForTool(item: WebviewToolCard): string | undefined {
  return item.planPath ?? asString(item.args?.path);
}

function createPlanTodosFromArgs(
  args: Record<string, unknown> | undefined,
  t: Translator,
): WebviewTodo[] | undefined {
  const todos = args?.todos;
  if (!Array.isArray(todos)) {
    return undefined;
  }
  const parsed = todos.flatMap((todo, index) => {
    if (typeof todo !== "object" || todo === null) {
      return [];
    }
    const entry = todo as Record<string, unknown>;
    const content = asString(entry.content) ?? t("todo.untitled", { number: index + 1 });
    const id = asString(entry.id) ?? `todo-${index + 1}`;
    const status =
      entry.status === "cancelled" ||
      entry.status === "completed" ||
      entry.status === "in_progress" ||
      entry.status === "pending"
        ? entry.status
        : "pending";
    return [{ content, id, status } satisfies WebviewTodo];
  });
  return parsed.length > 0 ? parsed : undefined;
}

export function createPlanFileCardFromTool(
  item: WebviewToolCard,
  options: {
    currentPlanId?: string | null;
    currentPlanState?: WebviewPlanFileState | null;
    planTodos?: WebviewTodo[];
  },
  t: Translator = defaultT,
): WebviewPlanFileCard | null {
  if (item.toolName !== "create_plan" || item.isError) {
    return null;
  }
  const creating = item.status === "running" || item.status === "streaming";
  if (!creating && item.status !== "complete") {
    return null;
  }
  const path = planPathForTool(item);
  if (!path) {
    return null;
  }
  const isActivePlan = !!item.planId && item.planId === options.currentPlanId;
  const argTodos = createPlanTodosFromArgs(item.args, t);
  const ambientTodos =
    options.planTodos && options.planTodos.length > 0
      ? options.planTodos
      : undefined;
  return {
    id: item.id,
    overview: item.planActivity?.overview ?? undefined,
    path,
    planId: item.planId ?? null,
    state: isActivePlan
      ? (options.currentPlanState ?? item.planActivity?.stateAfter ?? null)
      : (item.planActivity?.stateAfter ?? null),
    title: item.planActivity?.title ?? asString(item.args?.goal) ?? undefined,
    todos: isActivePlan ? (ambientTodos ?? argTodos) : argTodos,
    type: "plan",
  };
}

export interface PlanFileCardModelPicker {
  availableModelDetails?: Record<string, ModelPickerModel>;
  availableModels: readonly string[];
  buildModel?: string;
  onSelectContextWindow(modelId: string, contextWindow: number): void;
  onSelectThinkingLevel(modelId: string, level: string): void;
  onSelectSpeed(modelId: string, speed: Speed): void;
  onSetBuildModel(modelId: string): void;
  sessionContextWindow?: number | null;
  sessionModel?: string;
  sessionThinkingLevel?: string | null;
}

function PlanFileCardComponent({
  canBuild,
  creating = false,
  item,
  modelPicker,
  onBuild,
  onOpenPlanFile,
  planTodos = [],
}: {
  canBuild: boolean;
  creating?: boolean;
  item: WebviewPlanFileCard;
  modelPicker?: PlanFileCardModelPicker;
  onBuild(planId: string | null, path: string): void;
  onOpenPlanFile(path: string): void;
  planTodos?: WebviewTodo[];
}) {
  const t = useT();
  const locale = useLocale();
  const count = item.todos?.length ?? planTodos.length;
  const fileName = basename(item.path);
  const title = derivePlanTitle(item, fileName);
  const buildAllowed =
    canBuild && (item.state === "planning" || item.state === "pending");
  const selectedModelId = modelPicker?.buildModel || modelPicker?.sessionModel || null;
  const pickerModels = modelPicker
    ? buildPickerModels({
        activeModelId: modelPicker.sessionModel,
        availableModelDetails: modelPicker.availableModelDetails,
        availableModels: modelPicker.availableModels,
        selectedModelId,
        sessionContextWindow: modelPicker.sessionContextWindow,
        sessionThinkingLevel: modelPicker.sessionThinkingLevel,
      })
    : [];

  return (
    <section className="tc-card tc-plan-card" data-testid="plan-card">
      <button
        aria-label={t("plan.open")}
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
        aria-label={t("plan.open")}
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
        {t(pluralKey(locale, "todo.count.other", count), { count })}
      </div>
      <div className="tc-plan-card__footer">
        {creating ? (
          <button
            aria-busy="true"
            aria-label={t("plan.creating")}
            className="tc-plan-card__footer-link tc-plan-card__footer-link--busy"
            data-testid="view-plan-pending"
            disabled
            type="button"
          >
            <LoadingDots className="tc-plan-card__footer-dots" />
          </button>
        ) : (
          <button
            aria-label={t("plan.viewAria")}
            className="tc-plan-card__footer-link"
            data-testid="view-plan"
            onClick={() => onOpenPlanFile(item.path)}
            type="button"
          >
            {t("plan.view")}
          </button>
        )}
        {modelPicker ? (
          <ModelPicker
            className="tc-plan-model-picker"
            disabled={pickerModels.length === 0}
            label={t("model.label")}
            models={pickerModels}
            onSelectContextWindow={modelPicker.onSelectContextWindow}
            onSelectModel={modelPicker.onSetBuildModel}
            onSelectThinkingLevel={modelPicker.onSelectThinkingLevel}
            onSelectSpeed={modelPicker.onSelectSpeed}
            placement="above"
            selectedModelId={selectedModelId}
            testId="plan-card-build-model"
          />
        ) : null}
        <button
          className="tc-button tc-plan-build-button"
          data-testid="build-plan"
          disabled={!buildAllowed}
          onClick={() => onBuild(item.planId ?? null, item.path)}
          type="button"
        >
          {t(item.state === "pending" ? "common.resume" : "plan.build")}
        </button>
      </div>
    </section>
  );
}

function arePlanFileCardPropsEqual(
  previous: Readonly<Parameters<typeof PlanFileCardComponent>[0]>,
  next: Readonly<Parameters<typeof PlanFileCardComponent>[0]>,
): boolean {
  return (
    previous.canBuild === next.canBuild &&
    previous.creating === next.creating &&
    previous.item === next.item &&
    previous.modelPicker === next.modelPicker &&
    previous.onBuild === next.onBuild &&
    previous.onOpenPlanFile === next.onOpenPlanFile &&
    previous.planTodos === next.planTodos
  );
}

export const PlanFileCard = memo(PlanFileCardComponent, arePlanFileCardPropsEqual);
