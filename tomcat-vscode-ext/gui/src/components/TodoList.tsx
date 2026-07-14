import type { PlanTodo, PlanTodoStatus } from "../../../src/shared/planPreviewProtocol";

function TodoIcon({ status }: { status: PlanTodoStatus }) {
  const common = {
    "aria-hidden": true,
    className: "tc-plan-todo__icon",
    focusable: false,
    viewBox: "0 0 18 18",
  } as const;
  switch (status) {
    case "completed":
      return (
        <svg {...common} data-state="completed">
          <circle className="tc-plan-todo__icon-fill" cx="9" cy="9" r="8" />
          <path className="tc-plan-todo__icon-check" d="M5 9.3 L7.8 12.1 L13 6.4" fill="none" />
        </svg>
      );
    case "in_progress":
      return (
        <svg {...common} data-state="in_progress">
          <circle
            className="tc-plan-todo__icon-ring"
            cx="9"
            cy="9"
            fill="none"
            r="7.5"
            strokeDasharray="2.6 2.6"
          />
        </svg>
      );
    case "cancelled":
      return (
        <svg {...common} data-state="cancelled">
          <circle className="tc-plan-todo__icon-ring" cx="9" cy="9" fill="none" r="7.5" />
          <line className="tc-plan-todo__icon-slash" x1="4.5" x2="13.5" y1="13.5" y2="4.5" />
        </svg>
      );
    default:
      return (
        <svg {...common} data-state="pending">
          <circle className="tc-plan-todo__icon-ring" cx="9" cy="9" fill="none" r="7.5" />
        </svg>
      );
  }
}

export function TodoList({ todos }: { todos: PlanTodo[] }) {
  if (todos.length === 0) {
    return (
      <p className="tc-plan-todos__empty" data-testid="plan-todo-empty">
        No to-dos yet.
      </p>
    );
  }
  return (
    <ul className="tc-plan-todos" data-testid="plan-todo-list">
      {todos.map((todo) => (
        <li className="tc-plan-todo" data-status={todo.status} data-testid="plan-todo-item" key={todo.id}>
          <TodoIcon status={todo.status} />
          <span className="tc-plan-todo__content">{todo.content || todo.id}</span>
        </li>
      ))}
    </ul>
  );
}
