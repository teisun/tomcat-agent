import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionFileIntent, SessionFilesView } from "../../../src/shared/sessionFiles";
import { useActiveTodoProgress } from "../hooks/useActiveTodoProgress";
import type { WebviewPlanFileState, WebviewTodo } from "../types";
import { collapsedTitle, TodoItems } from "./TodoListWidget";
import { SessionFilesList } from "./SessionFilesList";

export interface SessionDockProps {
  sessionId: string;
  busy: boolean;
  commandPending?: boolean;
  planState?: WebviewPlanFileState | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
  files?: SessionFilesView;
  onIntent(intent: SessionFileIntent): void;
}

export function SessionDock({ sessionId, busy, commandPending, planState, planTodos, sessionTodos, files, onIntent }: SessionDockProps) {
  const progress = useActiveTodoProgress({ busy, planState, planTodos, sessionTodos });
  const hasTodos = busy && !!progress;
  const hasFiles = !!files?.files.length;
  const both = hasTodos && hasFiles;
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState<"todos" | "files">("todos");
  const selectedView = selected === "todos" && !hasTodos ? "files" : selected === "files" && !hasFiles && hasTodos ? "todos" : selected;
  const listRef = useRef<HTMLDivElement>(null);
  const todoSignature = useMemo(() => progress?.todos.map((t) => `${t.id}:${t.status}:${t.content}`).join("|") ?? "", [progress]);
  const previousTodo = useRef(todoSignature);
  const previousSource = useRef(files?.sourceTurnId);
  useEffect(() => {
    // Make fallback selection real: finishing Todos must not let the next
    // question silently switch the user away from the Files they are viewing.
    if ((hasTodos || hasFiles) && selected !== selectedView) setSelected(selectedView);
  }, [hasTodos, hasFiles, selected, selectedView]);
  useEffect(() => {
    if (previousTodo.current !== todoSignature && selectedView === "todos") setExpanded(false);
    previousTodo.current = todoSignature;
  }, [todoSignature, selectedView]);
  useEffect(() => {
    if (previousSource.current !== files?.sourceTurnId && listRef.current) listRef.current.scrollTop = 0;
    previousSource.current = files?.sourceTurnId;
  }, [files?.sourceTurnId]);
  function refresh() { onIntent({ messageId: crypto.randomUUID(), type: "refreshSessionFiles", data: { sessionId } }); }
  function choose(tab: "todos" | "files") { setSelected(tab); setExpanded(true); if (tab === "files") refresh(); }
  const toggle = () => {
    if (!both) setSelected(hasTodos ? "todos" : "files");
    if (!expanded && selectedView === "files") refresh();
    setExpanded(!expanded);
  };
  if (!hasTodos && !hasFiles) {
    if (!files?.error) return null;
    return <section className="tc-session-dock" data-testid="session-dock"><div className="tc-session-dock__error" role="status">Couldn't load file changes. <button type="button" onClick={refresh}>Retry</button></div></section>;
  }
  const tooltip = `Changes from the last editing turn${files?.error ? ` · Refresh failed: ${files.error}` : ""}`;
  const title = hasTodos && progress ? (expanded ? `Todos (${progress.current}/${progress.total})` : collapsedTitle(progress)) : `${files?.files.length ?? 0} Files`;
  return <section className="tc-session-dock" data-testid="session-dock" data-todo-available={hasTodos} data-source-turn={files?.sourceTurnId ?? ""}>
    {both ? <div className="tc-session-dock__header">
      <button aria-label={expanded ? "Collapse details" : "Expand details"} aria-expanded={expanded} aria-controls="session-dock-details" className="tc-session-dock__chevron" onClick={toggle} type="button"><span aria-hidden="true" className={`codicon codicon-chevron-${expanded ? "down" : "right"}`} /></button>
      <div aria-label="Session details" className="tc-session-dock__tabs" role="tablist">
        {(["todos", "files"] as const).map((tab) => <button key={tab} id={`dock-tab-${tab}`} role="tab" type="button" aria-selected={selectedView === tab} aria-controls="session-dock-details" tabIndex={selectedView === tab ? 0 : -1} title={tab === "files" ? tooltip : undefined} className="tc-session-dock__tab" onClick={() => choose(tab)} onKeyDown={(event) => {
          if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
          event.preventDefault(); const next = event.key === "Home" ? "todos" : event.key === "End" ? "files" : tab === "files" ? "todos" : "files";
          choose(next); document.getElementById(`dock-tab-${next}`)?.focus();
        }}>{tab === "todos" ? `Todos ${progress!.current}/${progress!.total}` : `Files ${files!.files.length}`}</button>)}
      </div>
    </div> : <button type="button" className="tc-session-dock__toggle" data-testid={hasTodos ? "todo-widget-toggle" : "files-toggle"} aria-label={hasTodos ? (expanded ? "Collapse todos" : "Expand todos") : (expanded ? "Collapse files" : "Expand files")} aria-expanded={expanded} aria-controls="session-dock-details" title={hasFiles ? tooltip : undefined} onClick={toggle}>
      <span aria-hidden="true" className={`codicon codicon-chevron-${expanded ? "down" : "right"}`} /><span data-testid={hasTodos ? "todo-widget-title" : "files-title"} className={`tc-session-dock__title${hasTodos && progress && !progress.isComplete ? " tc-loading-shimmer" : ""}`}>{title}</span>
    </button>}
    {expanded ? <div id="session-dock-details" role={both ? "tabpanel" : undefined} aria-labelledby={both ? `dock-tab-${selectedView}` : undefined}>
      {selectedView === "todos" && progress ? <TodoItems todos={progress.todos} /> : <SessionFilesList sessionId={sessionId} sourceTurnId={files!.sourceTurnId!} files={files!.files} busy={busy || !!commandPending} onIntent={onIntent} listRef={listRef} />}
    </div> : null}
  </section>;
}
