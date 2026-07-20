import { useEffect, useMemo, useState } from "react";

import { useActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import type { WebviewPlanState, WebviewTodo } from "../types";

function statusIconClass(status: WebviewTodo["status"]): string {
  switch (status) {
    case "completed":
      return "codicon-pass";
    case "in_progress":
      return "codicon-record";
    case "cancelled":
      return "codicon-close";
    case "pending":
    default:
      return "codicon-circle-outline";
  }
}

function statusClass(status: WebviewTodo["status"]): string {
  return `tc-todo-widget__status--${status.replace("_", "-")}`;
}

function collapsedTitle(progress: ReturnType<typeof useActiveTodoProgress>): string {
  if (progress.isComplete || !progress.activeTodo) {
    return `Todos (${progress.current}/${progress.total})`;
  }
  return `${progress.activeTodo.content} (${progress.current}/${progress.total})`;
}

export function TodoListWidget({
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
  const sourceKey = useMemo(
    () =>
      progress?.todos
        .map((todo) => `${todo.id}:${todo.status}:${todo.content}`)
        .join("|") ?? "working",
    [progress],
  );
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    setExpanded(false);
  }, [sourceKey]);

  if (!busy) {
    return null;
  }

  if (!progress) {
    return null;
  }

  const title = expanded
    ? `Todos (${progress.current}/${progress.total})`
    : collapsedTitle(progress);

  return (
    <section
      className={`tc-todo-widget${expanded ? " tc-todo-widget--expanded" : ""}`}
      data-testid="todo-widget"
    >
      <button
        aria-controls="tc-todo-widget-list"
        aria-expanded={expanded}
        aria-label={expanded ? "Collapse todos" : "Expand todos"}
        className="tc-todo-widget__toggle"
        data-testid="todo-widget-toggle"
        onClick={() => setExpanded((value) => !value)}
        type="button"
      >
        <div className="tc-todo-widget__title-section">
          <span
            aria-hidden="true"
            className={`tc-todo-widget__chevron codicon ${
              expanded ? "codicon-chevron-down" : "codicon-chevron-right"
            }`}
          />
          {!expanded && !progress.isComplete && progress.activeTodo ? (
            <span
              aria-hidden="true"
              className={`tc-todo-widget__status codicon ${statusClass(
                progress.activeTodo.status,
              )} ${statusIconClass(progress.activeTodo.status)}`}
            />
          ) : null}
          <span
            className={`tc-todo-widget__title${!progress.isComplete ? " tc-loading-shimmer" : ""}`}
            data-testid="todo-widget-title"
          >
            {title}
          </span>
        </div>
        <span aria-hidden="true" className="tc-todo-widget__list-icon codicon codicon-list-flat" />
      </button>
      {expanded ? (
        <ul
          className="tc-todo-widget__list"
          data-testid="todo-widget-list"
          id="tc-todo-widget-list"
        >
          {progress.todos.map((todo) => (
            <li
              className="tc-todo-widget__item"
              data-status={todo.status}
              data-testid="todo-widget-item"
              key={todo.id}
            >
              <span
                aria-hidden="true"
                className={`tc-todo-widget__status codicon ${statusClass(todo.status)} ${statusIconClass(
                  todo.status,
                )}`}
              />
              <span className="tc-todo-widget__item-label">{todo.content}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
