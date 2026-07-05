import type { WebviewTodo } from "../serveClient/sessionRouter";

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function parseTodoStatus(value: unknown): WebviewTodo["status"] | null {
  switch (value) {
    case "pending":
    case "in_progress":
    case "completed":
    case "cancelled":
      return value;
    default:
      return null;
  }
}

export function parseTodos(value: unknown): WebviewTodo[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    if (!isRecord(entry) || typeof entry.id !== "string" || typeof entry.content !== "string") {
      return [];
    }
    const status = parseTodoStatus(entry.status);
    if (!status) {
      return [];
    }
    return [{ content: entry.content, id: entry.id, status }];
  });
}
