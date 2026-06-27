import type { ActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import { shouldShowProgressRow, useActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import type { WebviewPlanState, WebviewTodo } from "../types";

export function ProgressRow({
  busy,
  planState,
  planTodos,
  sessionTodos,
}: {
  busy: boolean;
  planState?: WebviewPlanState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
}) {
  const progress = useActiveTodoProgress({
    busy,
    planState,
    planTodos,
    sessionTodos,
  });

  if (!shouldShowProgressRow(progress, busy)) {
    return null;
  }

  if (!progress) {
    return (
      <div className="tc-progress-row tc-progress-row--shimmer" data-testid="progress-row">
        <span aria-hidden="true" className="tc-progress-row__spinner codicon codicon-loading" />
        <p>Working…</p>
      </div>
    );
  }

  return (
    <div
      className={`tc-progress-row${progress.isComplete ? "" : " tc-progress-row--shimmer"}`}
      data-testid="progress-row"
    >
      <span
        aria-hidden="true"
        className={`tc-progress-row__spinner codicon ${
          progress.isComplete ? "codicon-check" : "codicon-loading"
        }`}
      />
      <p data-testid="progress-row-text">
        {progress.title} ({progress.current}/{progress.total})
      </p>
    </div>
  );
}
