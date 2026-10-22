import type { InitializeResult } from "./initialize";
import type { DisposableLike } from "./protocol";
import { DEFAULT_REQUEST_TIMEOUT_MS } from "./TomcatMessenger";
import type { TomcatMessenger, TomcatMessengerExit } from "./TomcatMessenger";
import { ServiceVersionMismatchError } from "./sharedConnection";

// Cold startup is the slowest serve operation. Keep its budget at least as
// large as a regular control request so a healthy, still-initializing process
// is not killed and restarted before it can answer.
export const DEFAULT_HANDSHAKE_TIMEOUT_MS = Math.max(
  60_000,
  DEFAULT_REQUEST_TIMEOUT_MS,
);
const DEFAULT_MAX_ATTEMPTS = 5;
const DEFAULT_RETRY_WINDOW_MS = 3 * 60_000;
const DETERMINISTIC_FAILURE_MAX_ELAPSED_MS = 2_000;
const RETRY_DELAYS_MS = [300, 800, 2_000, 4_000] as const;

export type ServeConnectionPhase =
  | "idle"
  | "starting"
  | "handshaking"
  | "backoff"
  | "ready"
  | "fatal"
  | "disposed";

export type ServeConnectionStatus =
  | "connecting"
  | "reconnecting"
  | "ready"
  | "failed";

export type ServeConnectionFailureKind =
  | "version_mismatch"
  | "executable_missing"
  | "deterministic_startup"
  | "handshake_timeout"
  | "retry_exhausted";

export interface ServeConnectionFailure {
  attempt: number;
  error: Error;
  kind: ServeConnectionFailureKind;
  stderr: string;
}

export interface ServeConnectionState {
  attempt: number;
  failure?: ServeConnectionFailure;
  phase: ServeConnectionPhase;
  result?: InitializeResult;
  status: ServeConnectionStatus;
}

type SupervisedMessenger = Pick<
  TomcatMessenger,
  | "dispose"
  | "isRunning"
  | "onExit"
  | "recentStderr"
  | "runServiceAction"
  | "restart"
  | "start"
  | "stop"
>;

type Attempt = {
  cycle: number;
  failed: boolean;
  number: number;
  startedAt: number;
};

type ReadyWaiter = {
  reject(error: Error): void;
  resolve(result: InitializeResult): void;
};

export interface ServeConnectionSupervisorOptions {
  /**
   * The only component allowed to start or restart the process. Other callers
   * may keep using the messenger for normal requests after `whenReady()`.
   */
  messenger: SupervisedMessenger;
  /**
   * Sends the initialize control request. The timeout is deliberately scoped to
   * the startup handshake; regular agent requests keep their existing timeout.
   */
  initialize(timeoutMs: number): Promise<InitializeResult>;
  isExecutableAvailable(): boolean;
  maxAttempts?: number;
  now?(): number;
  random?(): number;
  retryWindowMs?: number;
}

import { t } from "../shared/i18n";

const FAILURE_SUMMARY_MAX_CHARS = 240;

/** One line for the chat panel and notifications; the full stderr stays in the output log. */
export function summarizeServeFailure(failure: Pick<ServeConnectionFailure, "error" | "stderr">): string {
  if (failure.error instanceof ServiceVersionMismatchError) {
    return t("bridge.serviceVersionMismatch", { running: failure.error.running, client: failure.error.client });
  }
  const lines = failure.stderr.split(/\r?\n/u).map((line) => line.trim()).filter(Boolean);
  const summary = lines[lines.length - 1] ?? failure.error.message.replace(/\s+/gu, " ").trim();
  return summary.length > FAILURE_SUMMARY_MAX_CHARS
    ? `${summary.slice(0, FAILURE_SUMMARY_MAX_CHARS - 1)}…`
    : summary;
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function deferredWaiter(): {
  promise: Promise<InitializeResult>;
  waiter: ReadyWaiter;
} {
  let resolve!: (result: InitializeResult) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<InitializeResult>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, waiter: { reject, resolve } };
}

/**
 * Owns this window's shared-service connection (not the service process):
 *
 *   start -> initialize -> ready
 *                  | failure
 *                  v
 *       bounded backoff -> start
 *
 * It intentionally does not interpret user configuration. A process that
 * repeatedly exits quickly with the same stderr is surfaced verbatim instead
 * of being guessed as an API-key problem.
 */
export class ServeConnectionSupervisor {
  private readonly handshakeTimeoutMs = DEFAULT_HANDSHAKE_TIMEOUT_MS;
  private readonly maxAttempts: number;
  private readonly now: () => number;
  private readonly random: () => number;
  private readonly retryWindowMs: number;
  private readonly stateListeners = new Set<(state: ServeConnectionState) => void>();
  private readonly waiters = new Set<ReadyWaiter>();
  private activeAttempt: Attempt | undefined;
  private attemptTimes: number[] = [];
  private cycle = 0;
  private disposed = false;
  private everReady = false;
  private exitSubscription: DisposableLike;
  private lastQuickFailure: { stderr: string } | undefined;
  private readyResult: InitializeResult | undefined;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private state: ServeConnectionState = {
    attempt: 0,
    phase: "idle",
    status: "connecting",
  };

  constructor(private readonly options: ServeConnectionSupervisorOptions) {
    this.maxAttempts = options.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
    this.now = options.now ?? Date.now;
    this.random = options.random ?? Math.random;
    this.retryWindowMs = options.retryWindowMs ?? DEFAULT_RETRY_WINDOW_MS;
    this.exitSubscription = options.messenger.onExit((event) => this.handleUnexpectedExit(event));
  }

  get currentState(): Readonly<ServeConnectionState> {
    return this.state;
  }

  get isReady(): boolean {
    return this.state.phase === "ready";
  }

  onStateChange(listener: (state: ServeConnectionState) => void): DisposableLike {
    this.stateListeners.add(listener);
    return {
      dispose: () => this.stateListeners.delete(listener),
    };
  }

  whenReady(): Promise<InitializeResult> {
    if (this.disposed) {
      return Promise.reject(new Error("Tomcat connection supervisor is disposed"));
    }
    if (this.readyResult) {
      return Promise.resolve(this.readyResult);
    }
    if (this.state.phase === "fatal") {
      return Promise.reject(this.fatalError());
    }
    const { promise, waiter } = deferredWaiter();
    this.waiters.add(waiter);
    if (this.state.phase === "idle") {
      this.beginCycle(false);
    }
    return promise;
  }

  /** An explicit user or configuration action gets a new bounded retry budget. */
  reconnect(): Promise<InitializeResult> {
    if (this.disposed) {
      return Promise.reject(new Error("Tomcat connection supervisor is disposed"));
    }
    this.beginCycle(this.options.messenger.isRunning);
    return this.whenReady();
  }

  /** An explicit global restart; automatic recovery never calls this. */
  async restartService(): Promise<InitializeResult> {
    if (this.disposed) throw new Error("Tomcat connection supervisor is disposed");
    this.cancelRetry();
    const cycle = ++this.cycle;
    this.activeAttempt = undefined;
    this.readyResult = undefined;
    this.options.messenger.stop();
    this.transition({ attempt: 0, phase: "backoff", status: "reconnecting" });
    try {
      await this.options.messenger.runServiceAction("restart");
    } catch (error) {
      if (!this.disposed && cycle === this.cycle) {
        this.enterFatal({ attempt: 0, error: asError(error), kind: "deterministic_startup", stderr: this.options.messenger.recentStderr });
      }
      throw error;
    }
    if (this.disposed || cycle !== this.cycle) throw new Error("Tomcat service restart was superseded");
    return this.reconnect();
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.cancelRetry();
    this.exitSubscription.dispose();
    this.rejectWaiters(new Error("Tomcat connection supervisor is disposed"));
    this.transition({
      attempt: this.state.attempt,
      phase: "disposed",
      status: "failed",
    });
    this.options.messenger.dispose();
    this.stateListeners.clear();
  }

  private beginCycle(restartRunningProcess: boolean): void {
    this.cancelRetry();
    this.cycle += 1;
    this.activeAttempt = undefined;
    this.attemptTimes = [];
    this.lastQuickFailure = undefined;
    this.readyResult = undefined;

    if (!this.options.isExecutableAvailable()) {
      this.enterFatal({
        attempt: 0,
        error: new Error(t("bridge.cliMissing")),
        kind: "executable_missing",
        stderr: "",
      });
      return;
    }

    this.beginAttempt(restartRunningProcess, true);
  }

  private beginAttempt(restartRunningProcess: boolean, allowStart = false): void {
    if (this.disposed) {
      return;
    }
    const cycle = this.cycle;
    const attempt: Attempt = {
      cycle,
      failed: false,
      number: this.state.attempt + 1,
      startedAt: this.now(),
    };
    this.activeAttempt = attempt;
    this.transition({
      attempt: attempt.number,
      phase: "starting",
      status: this.connectionStatusWhileUnavailable(),
    });

    try {
      if (restartRunningProcess && this.options.messenger.isRunning) {
        void this.options.messenger.restart({ allowStart });
      } else {
        void this.options.messenger.start({ allowStart });
      }
    } catch (error) {
      this.handleAttemptFailure(attempt, asError(error));
      return;
    }

    // An injected/test child can emit exit synchronously from start(). Avoid
    // sending an initialize frame to a process that the exit handler already
    // declared dead.
    if (attempt.failed || cycle !== this.cycle || this.disposed) {
      return;
    }

    this.transition({
      attempt: attempt.number,
      phase: "handshaking",
      status: this.connectionStatusWhileUnavailable(),
    });
    void this.initializeAttempt(attempt);
  }

  private async initializeAttempt(attempt: Attempt): Promise<void> {
    try {
      const result = await this.options.initialize(this.handshakeTimeoutMs);
      if (attempt.failed || attempt.cycle !== this.cycle || this.disposed) {
        return;
      }
      this.activeAttempt = undefined;
      this.readyResult = result;
      this.everReady = true;
      this.transition({
        attempt: attempt.number,
        phase: "ready",
        result,
        status: "ready",
      });
      this.resolveWaiters(result);
    } catch (error) {
      if (
        !attempt.failed &&
        attempt.cycle === this.cycle &&
        !this.disposed &&
        this.options.messenger.isRunning
      ) {
        // The child did not crash. Restarting it would only discard its cold
        // start work and repeat the same timeout; surface one actionable
        // failure instead. An actual child exit is handled synchronously by
        // handleUnexpectedExit and keeps its bounded retry behaviour.
        this.enterFatal({
          attempt: attempt.number,
          error: asError(error),
          kind: "handshake_timeout",
          stderr: this.options.messenger.recentStderr.trim(),
        });
        return;
      }
      this.handleAttemptFailure(attempt, asError(error));
    }
  }

  private handleUnexpectedExit(event: TomcatMessengerExit): void {
    if (this.disposed) {
      return;
    }

    const error =
      event.error ??
      new Error(
        `tomcat serve exited (code=${String(event.code)}, signal=${String(event.signal)})`,
      );
    if (this.activeAttempt && !this.activeAttempt.failed) {
      this.handleAttemptFailure(this.activeAttempt, error, event.stderr);
      return;
    }

    // A process that was once Ready has just disappeared. Treat the exit as
    // failure #1 of a fresh normal recovery cycle, then reconnect automatically.
    if (this.state.phase === "ready") {
      this.beginCycleAfterReadyExit(error, event.stderr);
    }
  }

  private beginCycleAfterReadyExit(error: Error, stderr: string): void {
    this.cancelRetry();
    this.cycle += 1;
    this.readyResult = undefined;
    this.attemptTimes = [];
    this.lastQuickFailure = undefined;
    const attempt: Attempt = {
      cycle: this.cycle,
      failed: false,
      number: 1,
      startedAt: this.now(),
    };
    this.activeAttempt = attempt;
    this.handleAttemptFailure(attempt, error, stderr);
  }

  private handleAttemptFailure(
    attempt: Attempt,
    error: Error,
    stderr = "",
  ): void {
    if (attempt.failed || attempt.cycle !== this.cycle || this.disposed) {
      return;
    }
    attempt.failed = true;
    this.activeAttempt = undefined;
    const normalizedStderr = stderr.trim();
    const failure = this.classifyFailure(attempt, error, normalizedStderr);
    if (failure) {
      this.enterFatal(failure);
      return;
    }

    const now = this.now();
    this.attemptTimes.push(now);
    this.attemptTimes = this.attemptTimes.filter((time) => now - time <= this.retryWindowMs);
    if (this.attemptTimes.length >= this.maxAttempts) {
      this.enterFatal({
        attempt: attempt.number,
        error,
        kind: "retry_exhausted",
        stderr: normalizedStderr,
      });
      return;
    }

    const delayMs = this.retryDelayMs(attempt.number);
    this.transition({
      attempt: attempt.number,
      phase: "backoff",
      status: this.connectionStatusWhileUnavailable(),
    });
    const cycle = this.cycle;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = undefined;
      if (this.disposed || cycle !== this.cycle || this.state.phase !== "backoff") {
        return;
      }
      this.beginAttempt(true);
    }, delayMs);
    this.retryTimer.unref?.();
  }

  private classifyFailure(
    attempt: Attempt,
    error: Error,
    stderr: string,
  ): ServeConnectionFailure | undefined {
    if (error instanceof ServiceVersionMismatchError) {
      return { attempt: attempt.number, error, kind: "version_mismatch", stderr };
    }

    if (
      !this.options.isExecutableAvailable() ||
      /ENOENT/u.test(error.message)
    ) {
      return {
        attempt: attempt.number,
        error,
        kind: "executable_missing",
        stderr,
      };
    }

    const quickFailure =
      this.now() - attempt.startedAt <= DETERMINISTIC_FAILURE_MAX_ELAPSED_MS;
    if (
      quickFailure &&
      stderr.length > 0 &&
      this.lastQuickFailure?.stderr === stderr
    ) {
      return {
        attempt: attempt.number,
        error,
        kind: "deterministic_startup",
        stderr,
      };
    }
    this.lastQuickFailure =
      quickFailure && stderr.length > 0 ? { stderr } : undefined;
    return undefined;
  }

  private retryDelayMs(attempt: number): number {
    const base = RETRY_DELAYS_MS[Math.min(attempt - 1, RETRY_DELAYS_MS.length - 1)];
    // ±20% prevents many concurrently activated workspaces from retrying in lockstep.
    return Math.round(base * (0.8 + this.random() * 0.4));
  }

  private connectionStatusWhileUnavailable(): ServeConnectionStatus {
    return this.everReady ? "reconnecting" : "connecting";
  }

  private enterFatal(failure: ServeConnectionFailure): void {
    this.cancelRetry();
    if (this.options.messenger.isRunning) {
      this.options.messenger.stop();
    }
    this.transition({
      attempt: failure.attempt,
      failure,
      phase: "fatal",
      status: "failed",
    });
    this.rejectWaiters(failure.error);
  }

  private fatalError(): Error {
    return this.state.failure?.error ?? new Error(t("bridge.connectionFailed"));
  }

  private transition(next: ServeConnectionState): void {
    this.state = next;
    for (const listener of this.stateListeners) {
      listener(next);
    }
  }

  private cancelRetry(): void {
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = undefined;
    }
  }

  private resolveWaiters(result: InitializeResult): void {
    for (const waiter of this.waiters) {
      waiter.resolve(result);
    }
    this.waiters.clear();
  }

  private rejectWaiters(error: Error): void {
    for (const waiter of this.waiters) {
      waiter.reject(error);
    }
    this.waiters.clear();
  }
}
