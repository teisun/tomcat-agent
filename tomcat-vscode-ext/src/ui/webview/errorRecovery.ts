import type { WebviewIntent } from "./protocol";

/** The host owns the timer; the page receives only its deadline. Never persisted. */
export interface ErrorRecoveryClock {
  now(): number;
  setTimeout(callback: () => void, delayMs: number): ReturnType<typeof setTimeout>;
  clearTimeout(handle: ReturnType<typeof setTimeout>): void;
}
export interface PendingErrorRecovery {
  errorId: string;
  action: "retry" | "resume";
  dueAt: number;
}
const CLOCK: ErrorRecoveryClock = { now: () => Date.now(), setTimeout, clearTimeout };
const DELAY_MS = 5 * 60 * 1_000;

// Take over only when an intent invalidates automatic recovery before execution/deadline checks can see it.
const TAKEOVER = new Set<WebviewIntent["type"]>([
  "prompt", "steer", "retryUserMessage", "recoverErrorTurn", "runSlashCommand", "compact",
  "rewindAndResend", "restoreSessionFiles", "closeSession", "deleteSession",
]);
export function takesOverErrorRecovery(intent: WebviewIntent): boolean {
  return TAKEOVER.has(intent.type)
    || (intent.type === "queueAction" && intent.data.action === "send")
    || (intent.type === "setPlanMode" && intent.data.action === "build");
}

export class ErrorRecoveryWaits {
  private readonly awaitingHistory = new Set<string>();
  private readonly waiting = new Map<string, PendingErrorRecovery & { timer?: ReturnType<typeof setTimeout> }>();
  constructor(
    private readonly changed: (sessionId: string) => void,
    private readonly due: (sessionId: string, entry: PendingErrorRecovery) => void,
    private readonly clock: ErrorRecoveryClock = CLOCK,
  ) {}

  failed(sessionId: string): void {
    this.cancel(sessionId);
    this.awaitingHistory.add(sessionId);
  }

  discardPending(sessionId: string): void { this.awaitingHistory.delete(sessionId); }

  /** Called only after the current failed-idle history has actually been applied. */
  historyApplied(sessionId: string, candidate: { errorId: string; action: "retry" | "resume" } | null): void {
    if (!this.awaitingHistory.delete(sessionId) || !candidate) return;
    const entry: PendingErrorRecovery & { timer?: ReturnType<typeof setTimeout> } = {
      ...candidate, dueAt: this.clock.now() + DELAY_MS,
    };
    this.waiting.set(sessionId, entry);
    entry.timer = this.clock.setTimeout(() => {
      if (this.waiting.get(sessionId) !== entry) return;
      this.waiting.delete(sessionId);
      this.changed(sessionId);
      this.due(sessionId, entry);
    }, DELAY_MS);
    entry.timer.unref?.();
    this.changed(sessionId);
  }

  projection(sessionId: string): { errorId: string; dueAt: number } | undefined {
    const entry = this.waiting.get(sessionId);
    return entry ? { errorId: entry.errorId, dueAt: entry.dueAt } : undefined;
  }

  cancel(sessionId: string, expected?: { errorId: string; dueAt: number }): void {
    const entry = this.waiting.get(sessionId);
    if (expected && (!entry || entry.errorId !== expected.errorId || entry.dueAt !== expected.dueAt)) return;
    this.awaitingHistory.delete(sessionId);
    if (!entry) return;
    this.waiting.delete(sessionId);
    if (entry.timer !== undefined) this.clock.clearTimeout(entry.timer);
    this.changed(sessionId);
  }

  clear(): void {
    this.awaitingHistory.clear();
    for (const id of this.waiting.keys()) this.cancel(id);
  }
}
