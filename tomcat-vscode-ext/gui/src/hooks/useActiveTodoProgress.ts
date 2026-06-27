import type { WebviewPlanState, WebviewTodo } from "../types";

export interface ActiveTodoProgress {
  current: number;
  isComplete: boolean;
  phase: "completed" | "created" | "starting" | "updated";
  title: string;
  total: number;
}

function countByStatus(todos: WebviewTodo[], status: WebviewTodo["status"]): number {
  return todos.filter((todo) => todo.status === status).length;
}

function computeProgress(todos: WebviewTodo[]): ActiveTodoProgress | null {
  if (!todos.length) {
    return null;
  }

  const total = todos.length;
  const completed = countByStatus(todos, "completed");
  const inProgress = todos.find((todo) => todo.status === "in_progress");
  const current = inProgress ? completed + 1 : Math.max(1, completed);
  const title =
    inProgress?.content ??
    [...todos].reverse().find((todo) => todo.status === "completed")?.content ??
    todos[0]?.content ??
    "Todos";
  const isComplete = completed === total && total > 0 && !inProgress;

  return {
    current,
    isComplete,
    phase: isComplete ? "completed" : inProgress ? "starting" : "updated",
    title,
    total,
  };
}

export function useActiveTodoProgress(input: {
  busy: boolean;
  planState?: WebviewPlanState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
}): ActiveTodoProgress | null {
  const planStates: WebviewPlanState[] = ["planning", "executing", "pending", "completed"];
  if (input.planState && planStates.includes(input.planState)) {
    return computeProgress(input.planTodos);
  }

  if (input.sessionTodos.some((todo) => todo.status === "in_progress")) {
    return computeProgress(input.sessionTodos);
  }

  return null;
}

export function shouldShowProgressRow(
  progress: ActiveTodoProgress | null,
  busy: boolean,
): boolean {
  return busy && (progress !== null || busy);
}
