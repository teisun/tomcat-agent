import type { WebviewPlanState, WebviewTodo } from "../types";

export interface ActiveTodoProgress {
  activeTodo: WebviewTodo | null;
  current: number;
  isComplete: boolean;
  phase: "completed" | "starting" | "updated";
  title: string;
  total: number;
  todos: WebviewTodo[];
}

function countByStatus(todos: WebviewTodo[], status: WebviewTodo["status"]): number {
  return todos.filter((todo) => todo.status === status).length;
}

function selectCompletedTitle(todos: WebviewTodo[]): string | undefined {
  return [...todos].reverse().find((todo) => todo.status === "completed")?.content;
}

function computeProgress(todos: WebviewTodo[]): ActiveTodoProgress | null {
  if (!todos.length) {
    return null;
  }

  const total = todos.length;
  const completed = countByStatus(todos, "completed");
  const inProgress = todos.find((todo) => todo.status === "in_progress");
  const pending = todos.find((todo) => todo.status === "pending");
  const activeTodo = inProgress ?? pending ?? null;
  const current = inProgress ? completed + 1 : Math.max(1, completed);
  const title =
    inProgress?.content ??
    selectCompletedTitle(todos) ??
    todos[0]?.content ??
    "Todos";
  const isComplete = completed === total && total > 0 && !inProgress;

  return {
    activeTodo,
    current,
    isComplete,
    phase: isComplete ? "completed" : inProgress ? "starting" : "updated",
    title,
    total,
    todos,
  };
}

export function selectActiveTodoSource(input: {
  busy: boolean;
  planState?: WebviewPlanState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
}): WebviewTodo[] | null {
  const planStates: WebviewPlanState[] = ["planning", "executing", "pending", "completed"];
  if (input.planState && planStates.includes(input.planState)) {
    return input.planTodos;
  }

  if (input.sessionTodos.some((todo) => todo.status === "in_progress")) {
    return input.sessionTodos;
  }

  return null;
}

export function useActiveTodoProgress(input: {
  busy: boolean;
  planState?: WebviewPlanState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
}): ActiveTodoProgress | null {
  const todos = selectActiveTodoSource(input);
  return todos ? computeProgress(todos) : null;
}

export function shouldShowProgressRow(
  progress: ActiveTodoProgress | null,
  busy: boolean,
): boolean {
  return busy;
}
