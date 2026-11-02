import type { ErrorRecoveryClock } from "../errorRecovery";

/** Advance only the host recovery clock, never transports, heartbeats or browsers. */
export function recoveryClock() {
  let now = 1_800_000_000_000;
  let next = 0;
  const timers = new Map<number, { dueAt: number; callback(): void }>();
  const clock: ErrorRecoveryClock = {
    now: () => now,
    setTimeout(callback, ms) {
      const id = ++next;
      timers.set(id, { dueAt: now + ms, callback });
      return id as unknown as ReturnType<typeof setTimeout>;
    },
    clearTimeout(handle) { timers.delete(handle as unknown as number); },
  };
  return {
    clock,
    advance(ms: number) {
      now += ms;
      for (const [id, timer] of [...timers].sort((a, b) => a[1].dueAt - b[1].dueAt)) {
        if (timer.dueAt <= now && timers.delete(id)) timer.callback();
      }
    },
    callbacks: () => [...timers.values()].map(timer => timer.callback),
    pending: () => timers.size,
  };
}
