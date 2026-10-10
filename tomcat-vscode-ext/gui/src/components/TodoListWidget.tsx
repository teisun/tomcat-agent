import { t as defaultT, type Translator } from "../../../src/shared/i18n";
import type { ActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import type { WebviewTodo } from "../types";

function statusIconClass(status: WebviewTodo["status"]): string {
  switch (status) {
    case "completed": return "codicon-pass";
    case "in_progress": return "codicon-record";
    case "cancelled": return "codicon-close";
    default: return "codicon-circle-outline";
  }
}
export function collapsedTitle(progress: ActiveTodoProgress, t: Translator = defaultT): string {
  return progress.isComplete || !progress.activeTodo
    ? t("todo.progress", { current: progress.current, total: progress.total })
    : t("todo.activeProgress", { content: progress.activeTodo.content, current: progress.current, total: progress.total });
}

/** Todo presentation shared by the dock; no separate shell or expansion state. */
export function TodoItems({ todos }: { todos: WebviewTodo[] }) {
  return <ul className="tc-todo-widget__list" data-testid="todo-widget-list" id="tc-todo-widget-list">
    {todos.map((todo) => <li className="tc-todo-widget__item" data-status={todo.status} data-testid="todo-widget-item" key={todo.id}>
      <span aria-hidden="true" className={`tc-todo-widget__status codicon tc-todo-widget__status--${todo.status.replace("_", "-")} ${statusIconClass(todo.status)}`} />
      <span className="tc-todo-widget__item-label">{todo.content}</span>
    </li>)}
  </ul>;
}
