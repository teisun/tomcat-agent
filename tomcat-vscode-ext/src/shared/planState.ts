import type { ServePlanEvent } from "../serveClient/wire";

export type WebviewPlanFileState =
  | "planning"
  | "executing"
  | "pending"
  | "completed";

export type WebviewAgentMode = "chat" | "plan";

export function normalizePlanFileState(
  value: unknown,
): WebviewPlanFileState | null {
  switch (value) {
    case "planning":
    case "executing":
    case "pending":
    case "completed":
      return value;
    default:
      return null;
  }
}

export function planFileStateProgressLabel(
  state: WebviewPlanFileState | null,
  planId?: string | null,
): string {
  const suffix = planId ? ` (${planId})` : "";
  switch (state) {
    case "planning":
      return `Tomcat plan mode${suffix}`;
    case "executing":
      return `Tomcat executing plan${suffix}`;
    case "pending":
      return `Tomcat plan pending${suffix}`;
    case "completed":
      return `Tomcat completed plan${suffix}`;
    default:
      return "Tomcat plan state updated";
  }
}

export function planEventState(
  event: ServePlanEvent,
): WebviewPlanFileState | null {
  const explicit = normalizePlanFileState(
    "state" in event ? event.state : undefined,
  );
  if (explicit) {
    return explicit;
  }

  switch (event.type) {
    case "plan.build":
      return "executing";
    case "plan.complete":
      return "completed";
    case "plan.pending":
    case "plan.stalled":
      return "pending";
    case "plan.create":
    case "plan.update":
      return "planning";
    default:
      return null;
  }
}
