import type { ReactNode } from "react";
import type { SessionFileIntent, SessionFilesView } from "../../../src/shared/sessionFiles";
import { useActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import type { WebviewPlanFileState, WebviewTodo } from "../types";
import { collapsedTitle, TodoItems } from "./TodoListWidget";
import { DockSection } from "./DockSection";
import { SessionFilesDock } from "./SessionFilesDock";

export interface SessionDockProps {
  sessionId: string;
  busy: boolean;
  commandPending?: boolean;
  planState?: WebviewPlanFileState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
  files?: SessionFilesView;
  messages?: ReactNode;
  onIntent(intent: SessionFileIntent): void;
}

export function SessionDock({ sessionId, busy, commandPending, planState, planTodos, sessionTodos, files, messages, onIntent }: SessionDockProps) {
  const progress = useActiveTodoProgress({ busy, planState, planTodos, sessionTodos });
  const hasTodos = busy && !!progress;
  if (!messages && !hasTodos && !files?.files.length && !files?.error) return null;
  return <div className="tc-dock-stack" data-testid="session-dock" data-todo-available={hasTodos} data-source-turn={files?.sourceTurnId ?? ""}>
    {messages}
    <SessionFilesDock sessionId={sessionId} files={files} busy={busy || !!commandPending} onIntent={onIntent} />
    {hasTodos && progress ? <DockSection label="todos" title={expanded => expanded ? `Todos (${progress.current}/${progress.total})` : collapsedTitle(progress)} testId="todos-dock" toggleTestId="todo-widget-toggle" titleTestId="todo-widget-title"
      titleClassName={!progress.isComplete ? "tc-loading-shimmer" : undefined}><TodoItems todos={progress.todos} /></DockSection> : null}
  </div>;
}
