import { createContext, useContext } from 'react';
import { activeTask, type BackgroundTask, type ChildTask } from '../../../src/shared/backgroundTasks';
import { useT } from '../i18n/LocaleProvider';
import type { WebviewToolCard } from '../types';
import { DockSection } from './DockSection';

interface TaskView {
  rows: BackgroundTask[];
  stop?(row: BackgroundTask): void;
  open?(row: ChildTask): void;
  settings?(row: ChildTask): void;
  shellTitle?(row: BackgroundTask): string | undefined;
}
export const TasksContext = createContext<TaskView>({ rows: [] });

function TaskLine({ row, detail }: { row: BackgroundTask; detail?: boolean }) {
  const { stop, open, settings, shellTitle } = useContext(TasksContext);
  const t = useT();
  const title = row.kind === 'shell' ? shellTitle?.(row) || row.command : row.description;
  const role = row.kind === 'subagent' && (row.type === 'explorer' ? 'Explorer' : 'Plan Reviewer');
  const state = row.state[0].toUpperCase() + row.state.slice(1);
  return <li className="tc-background-task" data-task-id={row.id} data-state={row.state}>
    <div className="tc-background-task__header">
      {row.kind === 'subagent' && open
        ? <button type="button" data-testid={detail ? 'card-child-title' : 'dock-child-title'} className="tc-background-task__title" title={title} onClick={() => open(row)}>{title}</button>
        : <span className="tc-background-task__title" title={title}>{title}</span>}
      {row.kind === 'subagent' && (settings
        ? <button type="button" data-testid="child-role-settings" className="tc-background-task__role" onClick={() => settings(row)}>{role}</button>
        : <small className="tc-background-task__role">{role}</small>)}
      {activeTask(row) && stop && <button type="button" className="tc-background-task__stop" data-testid={`${detail ? 'card' : 'dock'}-stop-${row.kind}`} disabled={row.state === 'stopping'} aria-label={t('task.stop', { title })} onClick={() => stop(row)}>Stop</button>}
    </div>
    {detail && <small className="tc-background-task__activity">{activeTask(row) && row.kind === 'subagent' ? row.activity || state : state}</small>}
  </li>;
}

export function ReviewerTaskLine({ id }: { id: string }) {
  const row = useContext(TasksContext).rows.find(r => r.kind === 'subagent' && r.type === 'plan_reviewer' && r.id === id);
  return row ? <ul className="tc-task-list"><TaskLine row={row} detail /></ul> : null;
}

export function ChildTaskCard({ item }: { item: WebviewToolCard }) {
  const { rows } = useContext(TasksContext);
  const t = useT();
  const own = rows.filter(r => r.kind === 'subagent' && r.toolCallId === item.toolCallId);
  if (own.length) return <ul className="tc-task-list">{own.map(r => <TaskLine key={r.id} row={r} detail />)}</ul>;
  if (item.isError) return null; // The tool row already displays the dispatch error.
  const tasks = Array.isArray(item.args?.tasks) ? item.args.tasks : [];
  return <ul className="tc-task-list">{tasks.map((task, i) => <li className="tc-background-task" key={i}>
    <div className="tc-background-task__header"><span className="tc-background-task__title">{String(task?.description || task?.id || 'Explorer')}</span></div>
    <small className="tc-background-task__activity">{item.status === 'complete' ? 'Interrupted' : t('progress.working')}</small>
  </li>)}</ul>;
}

export function BackgroundDock() {
  const { rows } = useContext(TasksContext);
  const t = useT();
  return <>{(['subagent', 'shell'] as const).map(kind => {
    const active = rows.filter(r => r.kind === kind && activeTask(r));
    return active.length ? <DockSection key={kind} label={kind} title={`${t(`dock.label.${kind}`)} · ${active.length}`} defaultExpanded testId={`dock-${kind}`}>
      <ul className={`tc-task-list tc-task-list--${kind}`}>{active.map(r => <TaskLine key={r.id} row={r} />)}</ul>
    </DockSection> : null;
  })}</>;
}
