import type { WebviewPlanFileCard, WebviewTodo } from "../types";

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || filePath;
}

function todoStatusClass(status: WebviewTodo["status"]): string {
  return `tc-plan-todo--${status.replace("_", "-")}`;
}

export function PlanFileCard({
  item,
  onOpenPlanFile,
  planTodos = [],
}: {
  item: WebviewPlanFileCard;
  onOpenPlanFile(path: string): void;
  planTodos?: WebviewTodo[];
}) {
  return (
    <section className="tc-card tc-plan-card" data-testid="plan-card">
      <div className="tc-card__header">
        <h3>Plan file</h3>
        <span className="tc-chip">{item.state ? `Plan: ${item.state}` : "Plan"}</span>
      </div>
      <button
        aria-label="Open plan file"
        className="tc-plan-card__link"
        onClick={() => onOpenPlanFile(item.path)}
        type="button"
      >
        <strong>{basename(item.path)}</strong>
        <span>{item.path}</span>
      </button>
      {planTodos.length ? (
        <ul className="tc-plan-todos" data-testid="plan-todos">
          {planTodos.map((todo) => (
            <li
              className={`tc-plan-todo ${todoStatusClass(todo.status)}`}
              data-testid={`plan-todo-${todo.status}`}
              key={todo.id}
            >
              <span aria-hidden="true" className="tc-plan-todo__checkbox">
                {todo.status === "completed"
                  ? "☑"
                  : todo.status === "in_progress"
                    ? "◔"
                    : todo.status === "cancelled"
                      ? "☒"
                      : "☐"}
              </span>
              <span>{todo.content}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
