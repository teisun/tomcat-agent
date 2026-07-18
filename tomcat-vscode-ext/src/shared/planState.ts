import type { ServePlanEvent } from "../serveClient/wire";

export type ParticipantPlanState =
  | "chat"
  | "planning"
  | "executing"
  | "pending"
  | "completed";

export function normalizePlanState(
  value: unknown,
): ParticipantPlanState | null {
  switch (value) {
    case "chat":
    case "planning":
    case "executing":
    case "pending":
    case "completed":
      return value;
    default:
      return null;
  }
}

export function planStateProgressLabel(
  state: ParticipantPlanState | null,
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
    case "chat":
      return "Tomcat chat mode";
    default:
      return "Tomcat plan state updated";
  }
}

export function planEventState(
  event: ServePlanEvent,
): ParticipantPlanState | null {
  const explicit = normalizePlanState(
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
      return "pending";
    case "plan.enter":
    case "plan.create":
    case "plan.update":
      return "planning";
    case "plan.exit":
      return "chat";
    default:
      return null;
  }
}
