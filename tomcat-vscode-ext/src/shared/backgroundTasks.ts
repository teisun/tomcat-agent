import { isRecord } from './todos';

const states = ['running', 'stopping', 'completed', 'failed', 'stopped', 'interrupted'] as const;
export type TaskState = typeof states[number];
type Base = {id: string; state: TaskState; revision: number; startedAt: number; toolCallId?: string};
export type ChildTask = Base & {kind: 'subagent'; description: string; type: 'explorer' | 'plan_reviewer'; activity: string};
export type BackgroundTask = ChildTask | (Base & {kind: 'shell'; command: string});
export const activeTask = (r: BackgroundTask) => r.state === 'running' || r.state === 'stopping';
export function parseTask(v: unknown): BackgroundTask | undefined {
  if (!isRecord(v) || typeof v.id !== 'string' || !v.id || typeof v.revision !== 'number' || !Number.isSafeInteger(v.revision) || v.revision < 0 || !states.includes(v.state as TaskState)) return;
  const base = {id: v.id, revision: v.revision, state: v.state as TaskState, startedAt: typeof v.startedAt === 'number' ? v.startedAt : 0, toolCallId: typeof v.toolCallId === 'string' ? v.toolCallId : undefined};
  if (v.kind === 'shell' && typeof v.command === 'string') return {...base, kind: v.kind, command: v.command};
  if (v.kind === 'subagent' && (v.type === 'explorer' || v.type === 'plan_reviewer') && typeof v.description === 'string') return {...base, kind:v.kind, type:v.type, description:v.description, activity: typeof v.activity === 'string' ? v.activity : ''};
}
export function mergeTasks(current: BackgroundTask[] = [], incoming: BackgroundTask[]): BackgroundTask[] {
  const key = (r: BackgroundTask) => `${r.kind}:${r.id}`;
  const map = new Map(current.map(r => [key(r),r]));
  let changed = false;
  for (const r of incoming) {
    const old = map.get(key(r));
    if (old && (old.revision >= r.revision || (!activeTask(old) && activeTask(r)))) continue;
    map.set(key(r), r); changed = true;
  }
  return changed ? [...map.values()].sort((a,b) => a.startedAt - b.startedAt || a.id.localeCompare(b.id)) : current;
}
