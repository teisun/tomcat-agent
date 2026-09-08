import { randomUUID } from "node:crypto";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";

import type {
  AskQuestionResult,
  AskQuestionWireRequest,
  AskQuestionWireResponse,
  ControlCancelFrame,
  ControlRequestFrame,
  ControlResponseFrame,
  DisposableLike,
  RequestCommand,
} from "./protocol";
import {
  isControlFrame,
  isResponseFrame,
  isWireEvent,
  normalizeAskQuestionResponse,
  parseAskQuestionRequest,
} from "./protocol";
import type {
  ResponseFrame,
  ServeCommand,
  ServeEvent,
  GetMessagesParams,
  ListModelsPayload,
  ListProviderKeysPayload,
  ModelEntryInput,
  RemoveModelResponse,
  SetProviderKeyResponse,
  SetPlanModeAction,
  UpsertModelResponse,
} from "./wire";

export const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;

export interface TomcatMessengerLogger {
  debug?(message: string): void;
  info?(message: string): void;
  warn?(message: string): void;
  error?(message: string): void;
}

export interface TomcatMessengerOptions {
  executable: string;
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  extraArgs?: string[];
  requestTimeoutMs?: number;
  logger?: TomcatMessengerLogger;
  spawnFactory?: typeof spawn;
}

export interface SetPlanModeRequest {
  action: SetPlanModeAction;
  planId?: string | null;
  sessionId?: string | null;
}

export interface TomcatMessengerExit {
  code: number | null;
  signal: NodeJS.Signals | null;
  error?: Error;
  stderr: string;
}

export type ControlRequestHandlerResult =
  | {
      kind: "response";
      payload: unknown;
      sessionId?: string | null;
    }
  | {
      kind: "cancel";
      payload?: unknown;
      sessionId?: string | null;
    };

export type ControlRequestHandler = (
  frame: ControlRequestFrame,
  context: { generation: number; signal: AbortSignal },
) =>
  | Promise<ControlRequestHandlerResult | void>
  | ControlRequestHandlerResult
  | void;

type PendingResponse = {
  reject(error: Error): void;
  resolve(frame: ResponseFrame): void;
  timeout: NodeJS.Timeout;
};

type PendingControl = {
  reject(error: Error): void;
  resolve(frame: ControlResponseFrame | ControlCancelFrame): void;
  timeout: NodeJS.Timeout;
};

type TypedResponseFrame<TPayload> = Omit<ResponseFrame, "payload"> & {
  payload?: TPayload;
};

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function createDisposable(callback: () => void): DisposableLike {
  return {
    dispose: callback,
  };
}

type OwnedChild = {
  child: ChildProcessWithoutNullStreams;
  closed: Promise<void>;
  exited: boolean;
  stopping: boolean;
  killTimer?: NodeJS.Timeout;
  termTimer?: NodeJS.Timeout;
};

export class TomcatMessenger {
  private readonly controlHandlers = new Map<string, ControlRequestHandler>();
  private readonly activeControlRequests = new Map<string, AbortController>();
  private readonly controlRequestListeners = new Set<
    (frame: ControlRequestFrame) => void
  >();
  private readonly eventListeners = new Set<(event: ServeEvent) => void>();
  private readonly exitListeners = new Set<(event: TomcatMessengerExit) => void>();
  private readonly frameErrorListeners = new Set<(error: Error) => void>();
  private readonly pendingControl = new Map<string, PendingControl>();
  private readonly pendingResponses = new Map<string, PendingResponse>();
  private readonly stderrListeners = new Set<(chunk: string) => void>();
  private child?: ChildProcessWithoutNullStreams;
  // A child can exit/restart before its pipes close. Keep ownership until close.
  private readonly ownedChildren = new Set<OwnedChild>();
  private disposalPromise?: Promise<void>;
  private childAbort?: AbortController;
  private childGeneration = 0;
  private disposed = false;
  private stderrText = "";
  private stdoutBuffer = "";

  constructor(private readonly options: TomcatMessengerOptions) {}

  get isRunning(): boolean {
    return this.child !== undefined && this.child.exitCode === null && !this.child.killed;
  }

  get pid(): number | undefined {
    return this.child?.pid;
  }

  get recentStderr(): string {
    return this.stderrText;
  }

  start(): void {
    this.ensureNotDisposed();
    if (this.isRunning) {
      return;
    }

    const spawnFactory = this.options.spawnFactory ?? spawn;
    const child = spawnFactory(
      this.options.executable,
      ["serve", "--stdio", ...(this.options.extraArgs ?? [])],
      {
        cwd: this.options.cwd,
        env: {
          ...process.env,
          ...this.options.env,
        },
        stdio: "pipe",
      },
    );

    this.child = child;
    this.observeChildClose(child);
    const abortController = new AbortController();
    const generation = ++this.childGeneration;
    this.childAbort = abortController;
    this.stderrText = "";
    this.stdoutBuffer = "";

    child.stdout.on("data", (chunk: Buffer) => {
      if (this.child === child && this.childGeneration === generation) {
        this.handleStdoutChunk(chunk);
      }
    });
    child.stderr.on("data", (chunk: Buffer) => {
      if (this.child === child && this.childGeneration === generation) {
        this.handleStderrChunk(chunk);
      }
    });
    child.stdin.on("error", (error) => this.handleChildExit(child, {
      code: child.exitCode,
      error,
      signal: child.signalCode,
    }));
    child.stdout.on("error", (error) => this.handleChildExit(child, {
      code: child.exitCode,
      error,
      signal: child.signalCode,
    }));
    child.stderr.on("error", (error) => this.handleChildExit(child, {
      code: child.exitCode,
      error,
      signal: child.signalCode,
    }));
    child.on("error", (error) => {
      this.handleChildExit(child, {
        code: child.exitCode,
        error,
        signal: child.signalCode,
      });
    });
    child.on("exit", (code, signal) => {
      this.handleChildExit(child, { code, signal: signal ?? null });
    });
  }

  restart(): void {
    this.shutdown("TomcatMessenger restarting");
    this.start();
  }

  /**
   * Stops the current child without immediately starting another one. Lifecycle
   * recovery is owned by ServeConnectionSupervisor; callers should normally use
   * that supervisor rather than calling this directly.
   */
  stop(): void {
    this.shutdown("TomcatMessenger stopped");
  }

  updateOptions(options: Partial<TomcatMessengerOptions>): void {
    if (options.cwd !== undefined) {
      this.options.cwd = options.cwd;
    }
    if (options.env !== undefined) {
      this.options.env = options.env;
    }
    if (options.executable !== undefined) {
      this.options.executable = options.executable;
    }
    if (options.extraArgs !== undefined) {
      this.options.extraArgs = options.extraArgs;
    }
    if (options.logger !== undefined) {
      this.options.logger = options.logger;
    }
    if (options.requestTimeoutMs !== undefined) {
      this.options.requestTimeoutMs = options.requestTimeoutMs;
    }
    if (options.spawnFactory !== undefined) {
      this.options.spawnFactory = options.spawnFactory;
    }
  }

  dispose(): void {
    this.disposeWithGrace(0);
  }

  private disposeWithGrace(graceMs: number): void {
    if (this.disposed) {
      return;
    }

    this.disposed = true;
    this.shutdown("TomcatMessenger disposed", graceMs);
    for (const owned of this.ownedChildren) this.terminateChild(owned, 500, graceMs);
    this.controlHandlers.clear();
    this.controlRequestListeners.clear();
    this.eventListeners.clear();
    this.exitListeners.clear();
    this.frameErrorListeners.clear();
    this.stderrListeners.clear();
  }

  /**
   * Disables new work immediately, allows a short EOF grace period, then waits
   * for every owned generation's process AND stdio close. The budget includes escalation.
   * Repeated calls share the first call's promise/budget. Timeout is a failure,
   * not permission for a fixture to delete a directory a child may still use.
   */
  disposeAsync({ timeoutMs = 5_000 }: { timeoutMs?: number } = {}): Promise<void> {
    if (this.disposalPromise) return this.disposalPromise;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 2_147_483_647) {
      return Promise.reject(new RangeError("disposeAsync timeoutMs must be a positive timer duration"));
    }
    // Give EOF a short opportunity to settle checkpoint/title writes before a
    // signal interrupts the process. Synchronous dispose retains its old timing.
    this.disposeWithGrace(Math.min(500, timeoutMs / 4));
    const pending = [...this.ownedChildren];
    for (const owned of pending) this.terminateChild(owned, Math.min(1_000, timeoutMs / 2));
    let timer: NodeJS.Timeout;
    const deadline = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => {
        const remaining = [...this.ownedChildren];
        const detail = remaining.map(({ child }) =>
          `pid=${child.pid ?? "unknown"} exit=${child.exitCode} signal=${child.signalCode}`).join("; ");
        // Release our pipes as a last resort, but still report the missed deadline.
        for (const owned of remaining) {
          this.signalChild(owned, "SIGKILL");
          owned.child.stdin.destroy();
          owned.child.stdout.destroy();
          owned.child.stderr.destroy();
        }
        reject(new Error(`timed out closing tomcat serve after ${timeoutMs}ms: ${detail}`));
      }, timeoutMs);
    });
    this.disposalPromise = Promise.race([
      Promise.all(pending.map((owned) => owned.closed)).then(() => undefined),
      deadline,
    ]).finally(() => clearTimeout(timer));
    return this.disposalPromise;
  }

  private observeChildClose(child: ChildProcessWithoutNullStreams): void {
    let resolveClosed!: () => void;
    const owned: OwnedChild = {
      child,
      closed: new Promise<void>((resolve) => { resolveClosed = resolve; }),
      exited: false,
      stopping: false,
    };
    this.ownedChildren.add(owned);
    child.once("exit", () => { owned.exited = true; });
    child.once("close", () => {
      clearTimeout(owned.killTimer);
      clearTimeout(owned.termTimer);
      this.ownedChildren.delete(owned);
      resolveClosed();
    });
  }

  private signalChild(owned: OwnedChild, signal: NodeJS.Signals): void {
    if (owned.exited || !this.ownedChildren.has(owned)) return;
    try { owned.child.kill(signal); } catch (error) {
      this.log("warn", `failed to signal tomcat ${signal}: ${toError(error).message}`);
    }
  }

  private terminateChild(owned: OwnedChild, graceMs = 500, eofGraceMs = 0): void {
    if (!this.ownedChildren.has(owned)) return;
    if (!owned.stopping) {
      owned.stopping = true;
      try {
        if (!owned.child.stdin.destroyed) owned.child.stdin.end();
      } catch (error) {
        this.log("warn", `failed to close tomcat stdin: ${toError(error).message}`);
      }
      if (eofGraceMs > 0) {
        owned.termTimer = setTimeout(() => this.signalChild(owned, "SIGTERM"), eofGraceMs).unref();
      } else {
        this.signalChild(owned, "SIGTERM");
      }
    }
    if (!this.ownedChildren.has(owned) || owned.exited) return;
    clearTimeout(owned.killTimer);
    owned.killTimer = setTimeout(() => this.signalChild(owned, "SIGKILL"), graceMs).unref();
  }

  send(command: ServeCommand): void {
    this.start();
    this.writeCommand(command);
  }

  request(command: RequestCommand, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    const withId = this.withCommandId(command);
    return new Promise<ResponseFrame>((resolve, reject) => {
      const timeout = this.createTimeout(
        timeoutMs,
        () => {
          this.pendingResponses.delete(withId.id);
          reject(new Error(`Timed out waiting for response ${withId.id}`));
        },
      );

      this.pendingResponses.set(withId.id, {
        reject,
        resolve,
        timeout,
      });

      try {
        this.send(withId);
      } catch (error) {
        clearTimeout(timeout);
        this.pendingResponses.delete(withId.id);
        reject(toError(error));
      }
    });
  }

  requestControl(
    command: Extract<ServeCommand, { type: "control_request" }>,
    timeoutMs = this.timeoutMs(),
  ): Promise<ControlResponseFrame | ControlCancelFrame> {
    return new Promise((resolve, reject) => {
      const timeout = this.createTimeout(
        timeoutMs,
        () => {
          this.pendingControl.delete(command.requestId);
          reject(
            new Error(`Timed out waiting for control response ${command.requestId}`),
          );
        },
      );

      this.pendingControl.set(command.requestId, {
        reject,
        resolve,
        timeout,
      });

      try {
        this.send(command);
      } catch (error) {
        clearTimeout(timeout);
        this.pendingControl.delete(command.requestId);
        reject(toError(error));
      }
    });
  }

  sendListModels(timeoutMs = this.timeoutMs()): Promise<TypedResponseFrame<ListModelsPayload>> {
    return this.request(
      {
        type: "list_models",
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<ListModelsPayload>>;
  }

  sendUpsertModel(
    model: ModelEntryInput,
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<UpsertModelResponse>> {
    return this.request(
      {
        model,
        type: "upsert_model",
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<UpsertModelResponse>>;
  }

  sendListConnectors(timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ type: "list_connectors" }, timeoutMs);
  }

  sendListConnectorTools(name: string, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, type: "list_connector_tools" }, timeoutMs);
  }

  sendAddConnector(input: {
    name: string;
    command: string;
    args: string[];
    url?: string | null;
    headers?: Record<string, string>;
    auth?: "none" | "bearer" | "oauth";
    env?: Record<string, string>;
    oauth?: unknown;
    scope?: "user" | "workspace" | null;
  }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ ...input, type: "add_connector" }, timeoutMs);
  }

  sendRemoveConnector(name: string, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, type: "remove_connector" }, timeoutMs);
  }

  sendReloadConnector(timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ type: "reload_connector" }, timeoutMs);
  }
  sendSetConnectorTrust(name: string, trusted: boolean, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, trusted, type: "set_connector_trust" }, timeoutMs);
  }

  sendTestConnector(name: string, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, type: "test_connector" }, timeoutMs);
  }

  sendLoginConnector(name: string, timeoutMs = 300_000): Promise<ResponseFrame> {
    return this.request({ name, type: "login_connector" }, timeoutMs);
  }

  sendLogoutConnector(name: string, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, type: "logout_connector" }, timeoutMs);
  }

  sendCancelLoginConnector(name: string, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ name, type: "cancel_login_connector" }, timeoutMs);
  }
  sendSetConnectorToolFilter(
    name: string,
    include: string[],
    exclude: string[],
    scope: "user" | "workspace" = "workspace",
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request({ exclude, include, name, scope, type: "set_connector_tool_filter" }, timeoutMs);
  }


  sendRemoveModel(
    modelId: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<RemoveModelResponse>> {
    return this.request(
      {
        modelId,
        type: "remove_model",
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<RemoveModelResponse>>;
  }

  sendSetProviderKey(
    envName: string,
    value: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<SetProviderKeyResponse>> {
    return this.request(
      {
        envName,
        type: "set_provider_key",
        value,
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<SetProviderKeyResponse>>;
  }

  sendListProviderKeys(timeoutMs = this.timeoutMs()): Promise<TypedResponseFrame<ListProviderKeysPayload>> {
    return this.request(
      {
        type: "list_provider_keys",
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<ListProviderKeysPayload>>;
  }

  sendSetModel(
    sessionId: string | null | undefined,
    model: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request(
      {
        model,
        sessionId,
        type: "set_model",
      },
      timeoutMs,
    );
  }

  sendSetThinkingLevel(
    sessionId: string | null | undefined,
    model: string,
    level: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request(
      {
        level,
        model,
        sessionId,
        type: "set_thinking_level",
      },
      timeoutMs,
    );
  }

  sendSetContextWindow(
    sessionId: string | null | undefined,
    model: string,
    contextWindow: number,
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request(
      {
        contextWindow,
        model,
        sessionId,
        type: "set_context_window",
      },
      timeoutMs,
    );
  }

  sendSetPlanMode(
    command: SetPlanModeRequest,
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request(
      {
        action: command.action,
        planId: command.planId,
        sessionId: command.sessionId,
        type: "set_plan_mode",
      },
      timeoutMs,
    );
  }

  sendGetMessages(
    sessionId: string | null | undefined,
    params: GetMessagesParams = {},
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request(
      {
        params,
        sessionId,
        type: "get_messages",
      },
      timeoutMs,
    );
  }

  sendControlResponse(requestId: string, sessionId: string | null | undefined, payload: unknown): void {
    this.send({
      payload,
      requestId,
      sessionId,
      type: "control_response",
    });
  }

  sendControlCancel(requestId: string, sessionId: string | null | undefined, payload: unknown = null): void {
    this.send({
      payload,
      requestId,
      sessionId,
      type: "control_cancel",
    });
  }

  registerControlRequestHandler(
    subtype: string,
    handler: ControlRequestHandler,
  ): DisposableLike {
    this.controlHandlers.set(subtype, handler);
    return createDisposable(() => {
      if (this.controlHandlers.get(subtype) === handler) {
        this.controlHandlers.delete(subtype);
      }
    });
  }

  registerAskQuestionHandler(
    handler: (
      request: AskQuestionWireRequest,
      frame: ControlRequestFrame,
      context: { generation: number; signal: AbortSignal },
    ) => Promise<AskQuestionResult | AskQuestionWireResponse> | AskQuestionResult | AskQuestionWireResponse,
  ): DisposableLike {
    return this.registerControlRequestHandler("ask_question", async (frame, context) => {
      const request = parseAskQuestionRequest(frame.payload);
      const response = await handler(request, frame, context);
      return {
        kind: "response",
        payload: normalizeAskQuestionResponse(request.requestId, response),
        sessionId: frame.sessionId,
      };
    });
  }

  onControlRequest(listener: (frame: ControlRequestFrame) => void): DisposableLike {
    this.controlRequestListeners.add(listener);
    return createDisposable(() => {
      this.controlRequestListeners.delete(listener);
    });
  }

  onEvent(listener: (event: ServeEvent) => void): DisposableLike {
    this.eventListeners.add(listener);
    return createDisposable(() => {
      this.eventListeners.delete(listener);
    });
  }

  onExit(listener: (event: TomcatMessengerExit) => void): DisposableLike {
    this.exitListeners.add(listener);
    return createDisposable(() => {
      this.exitListeners.delete(listener);
    });
  }

  onFrameError(listener: (error: Error) => void): DisposableLike {
    this.frameErrorListeners.add(listener);
    return createDisposable(() => {
      this.frameErrorListeners.delete(listener);
    });
  }

  onStderr(listener: (chunk: string) => void): DisposableLike {
    this.stderrListeners.add(listener);
    return createDisposable(() => {
      this.stderrListeners.delete(listener);
    });
  }

  private ensureNotDisposed(): void {
    if (this.disposed) {
      throw new Error("TomcatMessenger has been disposed");
    }
  }

  private timeoutMs(): number {
    return this.options.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  }

  private withCommandId(command: RequestCommand): RequestCommand & { id: string } {
    if (command.id) {
      return command as RequestCommand & { id: string };
    }

    return {
      ...command,
      id: `${command.type}-${randomUUID()}`,
    };
  }

  private createTimeout(
    timeoutMs: number,
    onTimeout: () => void,
  ): NodeJS.Timeout {
    return setTimeout(() => {
      onTimeout();
    }, timeoutMs).unref();
  }

  private writeCommand(command: ServeCommand): void {
    const child = this.child;
    if (!child || child.stdin.destroyed) {
      throw new Error("tomcat serve process is not writable");
    }

    const line = `${JSON.stringify(command)}\n`;
    child.stdin.write(line, "utf8");
  }

  private shutdown(reason: string, eofGraceMs = 0): void {
    this.rejectPending(new Error(reason));

    const child = this.child;
    this.childAbort?.abort(reason);
    for (const abort of this.activeControlRequests.values()) {
      abort.abort(reason);
    }
    this.activeControlRequests.clear();
    this.childAbort = undefined;
    this.child = undefined;
    this.stdoutBuffer = "";
    this.stderrText = "";

    if (!child) {
      return;
    }

    // Keep close/error observers and continue draining old-generation output.
    // Existing identity/generation checks prevent stale data reaching the UI.
    for (const owned of this.ownedChildren) {
      if (owned.child === child) this.terminateChild(owned, 1_000, eofGraceMs);
    }
  }

  private rejectPending(error: Error): void {
    for (const [id, pending] of this.pendingResponses) {
      clearTimeout(pending.timeout);
      pending.reject(error);
      this.pendingResponses.delete(id);
    }

    for (const [requestId, pending] of this.pendingControl) {
      clearTimeout(pending.timeout);
      pending.reject(error);
      this.pendingControl.delete(requestId);
    }
  }

  private handleStdoutChunk(chunk: Buffer): void {
    this.stdoutBuffer += chunk.toString("utf8");

    while (true) {
      const newlineIndex = this.stdoutBuffer.indexOf("\n");
      if (newlineIndex === -1) {
        return;
      }

      const rawLine = this.stdoutBuffer.slice(0, newlineIndex).replace(/\r$/, "");
      this.stdoutBuffer = this.stdoutBuffer.slice(newlineIndex + 1);

      if (!rawLine.trim()) {
        continue;
      }

      this.handleStdoutLine(rawLine);
    }
  }

  private handleStdoutLine(line: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch (error) {
      this.emitFrameError(new Error(`Failed to parse NDJSON line: ${toError(error).message}`));
      return;
    }

    if (isResponseFrame(parsed)) {
      this.handleResponseFrame(parsed);
      return;
    }

    if (isControlFrame(parsed)) {
      this.handleControlFrame(parsed);
      return;
    }

    if (isWireEvent(parsed)) {
      for (const listener of this.eventListeners) {
        listener(parsed);
      }
      return;
    }

    this.emitFrameError(new Error(`Unknown serve frame shape: ${line}`));
  }

  private handleStderrChunk(chunk: Buffer): void {
    const text = chunk.toString("utf8");
    this.stderrText = `${this.stderrText}${text}`.slice(-16_384);
    for (const listener of this.stderrListeners) {
      listener(text);
    }
  }

  private handleResponseFrame(frame: ResponseFrame): void {
    const responseId = frame.id ?? undefined;
    if (!responseId) {
      this.log("warn", "received response frame without id");
      return;
    }

    const pending = this.pendingResponses.get(responseId);
    if (!pending) {
      this.log("debug", `dropping unknown response frame ${responseId}`);
      return;
    }

    clearTimeout(pending.timeout);
    this.pendingResponses.delete(responseId);
    pending.resolve(frame);
  }

  private handleControlFrame(frame: ControlRequestFrame | ControlResponseFrame | ControlCancelFrame): void {
    if (frame.type === "control_request") {
      for (const listener of this.controlRequestListeners) {
        listener(frame);
      }

      const handler = this.controlHandlers.get(frame.subtype);
      if (handler) {
        const abort = new AbortController();
        this.activeControlRequests.set(frame.requestId, abort);
        const parentSignal = this.childAbort?.signal;
        const abortForChildExit = () => abort.abort();
        parentSignal?.addEventListener("abort", abortForChildExit, { once: true });
        void this.runControlHandler(handler, frame, this.childGeneration, abort.signal)
          .finally(() => {
            parentSignal?.removeEventListener("abort", abortForChildExit);
            if (this.activeControlRequests.get(frame.requestId) === abort) {
              this.activeControlRequests.delete(frame.requestId);
            }
          });
      }
      return;
    }

    if (frame.type === "control_cancel") {
      const active = this.activeControlRequests.get(frame.requestId);
      if (!active) {
        this.log("debug", `dropping unknown control cancel ${frame.requestId}`);
        return;
      }
      active.abort();
      return;
    }

    const pending = this.pendingControl.get(frame.requestId);
    if (!pending) {
      this.log("debug", `dropping unknown control frame ${frame.requestId}`);
      return;
    }

    clearTimeout(pending.timeout);
    this.pendingControl.delete(frame.requestId);
    pending.resolve(frame);
  }

  private async runControlHandler(
    handler: ControlRequestHandler,
    frame: ControlRequestFrame,
    generation: number,
    signal?: AbortSignal,
  ): Promise<void> {
    if (!signal) return;
    try {
      const result = await handler(frame, { generation, signal });
      if (signal.aborted || generation !== this.childGeneration || !this.child) return;
      if (!result) {
        return;
      }

      if (result.kind === "cancel") {
        this.sendControlCancel(
          frame.requestId,
          result.sessionId ?? frame.sessionId,
          result.payload ?? null,
        );
        return;
      }

      this.sendControlResponse(
        frame.requestId,
        result.sessionId ?? frame.sessionId,
        result.payload,
      );
    } catch (error) {
      if (signal.aborted || generation !== this.childGeneration || !this.child) return;
      this.emitFrameError(toError(error));
      this.sendControlCancel(frame.requestId, frame.sessionId, null);
    }
  }

  private handleChildExit(
    child: ChildProcessWithoutNullStreams,
    event: { code: number | null; signal: NodeJS.Signals | null; error?: Error },
  ): void {
    if (this.child !== child) {
      return;
    }

    const error =
      event.error ??
      new Error(
        `tomcat serve exited (code=${String(event.code)}, signal=${String(event.signal)})`,
      );

    this.childAbort?.abort(error);
    this.childAbort = undefined;
    this.child = undefined;
    this.stdoutBuffer = "";

    this.rejectPending(error);

    if (event.error) {
      for (const owned of this.ownedChildren) {
        if (owned.child === child) this.terminateChild(owned);
      }
    }

    const payload: TomcatMessengerExit = {
      code: event.code,
      error: event.error,
      signal: event.signal,
      stderr: this.stderrText,
    };
    for (const listener of this.exitListeners) {
      listener(payload);
    }
  }

  private emitFrameError(error: Error): void {
    this.log("warn", error.message);
    for (const listener of this.frameErrorListeners) {
      listener(error);
    }
  }

  private log(level: keyof TomcatMessengerLogger, message: string): void {
    this.options.logger?.[level]?.(message);
  }
}
