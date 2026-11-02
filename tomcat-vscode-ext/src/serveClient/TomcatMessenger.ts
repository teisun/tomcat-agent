import { randomUUID } from "node:crypto";
import { connectSharedService, runSharedServiceAction, type SharedConnection, type SharedConnectionFactory } from "./sharedConnection";

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
  ConnectorReloadReceipt,
  ListConnectorToolsPayload,
  ListModelsPayload,
  ListProviderKeysPayload,
  ListRemoteModelsPayload,
  ProjectTrustPayload,
  ModelEntryInput,
  RemoveModelResponse,
  RemoveProviderResponse,
  SetConnectorToolEnabledResponse,
  SetProviderKeyResponse,
  SetPlanModeAction,
  UpsertModelResponse,
  UpsertModelsResponse,
} from "./wire";

import type { Speed } from "../shared/modelSpeed";
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
  connectFactory?: SharedConnectionFactory;
}

export interface SetDefaultModelResponse {
  envOverride: boolean;
  modelId: string;
  warnings: string[];
  /** False when `onlyIfUnset` found a default already on disk. */
  written?: boolean;
}

export interface SetUtilityModelResponse {
  envOverride: boolean;
  modelId: string;
  warnings: string[];
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

type OwnedConnection = {
  abort: AbortController;
  closed: Promise<void>;
  connection?: SharedConnection;
  ready?: Promise<void>;
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
  private active?: OwnedConnection;
  // Reconnecting does not erase old connections still waiting for server cleanup.
  private readonly ownedConnections = new Set<OwnedConnection>();
  private disposalPromise?: Promise<void>;
  private connectionGeneration = 0;
  private disposed = false;
  private stderrText = "";
  private stdoutBuffer = "";

  constructor(private readonly options: TomcatMessengerOptions) {}

  get isRunning(): boolean {
    return this.active !== undefined && !this.active.abort.signal.aborted;
  }

  get pid(): number | undefined {
    return this.active?.connection?.info.pid;
  }

  get cwd(): string | undefined {
    return this.options.cwd;
  }

  get recentStderr(): string {
    return this.stderrText;
  }

  start({ allowStart = true }: { allowStart?: boolean } = {}): Promise<void> {
    this.ensureNotDisposed();
    if (this.active) return this.active.ready!;
    let resolveClosed!: () => void;
    const owned: OwnedConnection = {
      abort: new AbortController(),
      closed: new Promise<void>((resolve) => { resolveClosed = resolve; }),
    };
    this.active = owned;
    this.ownedConnections.add(owned);
    this.connectionGeneration += 1;
    this.stderrText = "";
    this.stdoutBuffer = "";
    const closed = () => {
      this.ownedConnections.delete(owned);
      resolveClosed();
    };
    owned.ready = Promise.resolve().then(() => {
      owned.abort.signal.throwIfAborted();
      return (this.options.connectFactory ?? connectSharedService)({
        ...this.options,
        allowStart,
        signal: owned.abort.signal,
        onStderr: (chunk) => { if (this.active === owned) this.handleStderrChunk(chunk); },
      });
    }).then((connection) => {
      owned.connection = connection;
      const { socket } = connection;
      socket.once("close", () => { closed(); this.handleConnectionExit(owned); });
      socket.on("error", (error) => this.handleConnectionExit(owned, error));
      // Decode at the byte-stream boundary, including split UTF-8 characters.
      socket.setEncoding("utf8");
      socket.on("data", (chunk: string) => {
        if (this.active === owned) this.handleStdoutChunk(chunk);
      });
      // Closing the local-handshake readline interface pauses its input. Merely
      // attaching a data listener does not resume an explicitly paused socket.
      socket.resume();
      if (socket.destroyed || socket.readableEnded) throw new Error("Tomcat shared connection already closed");
      if (this.active !== owned || owned.abort.signal.aborted) socket.end();
    }).catch((error: unknown) => {
      closed();
      this.handleConnectionExit(owned, toError(error));
      throw error;
    });
    // start() is also called by lifecycle notifications which do not await it.
    // Errors still reject waiting requests and go through the normal onExit path.
    void owned.ready.catch(() => undefined);
    return owned.ready;
  }

  restart(options?: { allowStart?: boolean }): Promise<void> {
    this.shutdown("TomcatMessenger reconnecting");
    return this.start(options);
  }

  /** Disconnect only this window. Never send a service stop or signal its PID. */
  stop(): void {
    this.shutdown("TomcatMessenger disconnected");
  }

  /** Explicit service administration; its command label explains the all-tasks impact. */
  runServiceAction(action: "stop" | "restart"): Promise<void> {
    this.ensureNotDisposed();
    return runSharedServiceAction({
      ...this.options,
      signal: new AbortController().signal,
      onStderr: (chunk) => this.handleStderrChunk(chunk),
    }, action);
  }

  updateOptions(options: Partial<TomcatMessengerOptions>): void {
    if ("cwd" in options) {
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
    if (options.connectFactory !== undefined) {
      this.options.connectFactory = options.connectFactory;
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.shutdown("TomcatMessenger disposed");
    for (const owned of this.ownedConnections) {
      owned.abort.abort();
      owned.connection?.socket.end();
    }
    this.controlHandlers.clear();
    this.controlRequestListeners.clear();
    this.eventListeners.clear();
    this.exitListeners.clear();
    this.frameErrorListeners.clear();
    this.stderrListeners.clear();
  }

  /** Wait for EOF cleanup on every connection, without stopping the shared service. */
  disposeAsync({ timeoutMs = 5_000 }: { timeoutMs?: number } = {}): Promise<void> {
    if (this.disposalPromise) return this.disposalPromise;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 2_147_483_647) {
      return Promise.reject(new RangeError("disposeAsync timeoutMs must be a positive timer duration"));
    }
    this.dispose();
    const pending = [...this.ownedConnections];
    let timer: NodeJS.Timeout;
    const deadline = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => {
        for (const owned of this.ownedConnections) owned.connection?.socket.destroy();
        reject(new Error(`timed out closing Tomcat connections after ${timeoutMs}ms; service cleanup is unconfirmed`));
      }, timeoutMs);
    });
    this.disposalPromise = Promise.race([
      Promise.all(pending.map((owned) => owned.closed)).then(() => undefined),
      deadline,
    ]).finally(() => clearTimeout(timer));
    return this.disposalPromise;
  }

  send(command: ServeCommand): void {
    this.scheduleWrite(command);
  }

  private scheduleWrite(command: ServeCommand, stillPending: () => boolean = () => true): void {
    // Requests may bootstrap a fresh messenger, but never resurrect a stopped
    // service after a disconnect. Explicit lifecycle actions can opt in again.
    void this.start({ allowStart: this.connectionGeneration === 0 });
    const owned = this.active!;
    const write = () => {
      if (this.active === owned && !owned.abort.signal.aborted && stillPending()) this.writeCommand(command);
    };
    if (owned.connection) write();
    else void owned.ready!.then(write).catch((error: unknown) => this.handleConnectionExit(owned, toError(error)));
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
        this.scheduleWrite(withId, () => this.pendingResponses.get(withId.id)?.resolve === resolve);
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
        this.scheduleWrite(command, () => this.pendingControl.get(command.requestId)?.resolve === resolve);
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

  sendUpsertModels(
    models: ModelEntryInput[],
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<UpsertModelsResponse>> {
    return this.request({ models, type: "upsert_models" }, timeoutMs) as Promise<TypedResponseFrame<UpsertModelsResponse>>;
  }

  /** A failed fetch resolves with `success: false` and a `RemoteModelsFailure` payload. */
  sendListRemoteModels(
    input: { api: string; apiKey?: string; apiKeyEnv?: string; baseUrl: string },
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<ListRemoteModelsPayload>> {
    return this.request({ ...input, type: "list_remote_models" }, timeoutMs) as Promise<TypedResponseFrame<ListRemoteModelsPayload>>;
  }

  sendRemoveProvider(
    keyEnv: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<RemoveProviderResponse>> {
    return this.request({ keyEnv, type: "remove_provider" }, timeoutMs) as Promise<TypedResponseFrame<RemoveProviderResponse>>;
  }

  sendSetUtilityModel(
    modelId: string,
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<SetUtilityModelResponse>> {
    return this.request({ modelId, type: "set_utility_model" }, timeoutMs) as Promise<TypedResponseFrame<SetUtilityModelResponse>>;
  }

  sendListConnectors(
    context: { workspaceRoot?: string | null },
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request({ context, type: "list_connectors" }, timeoutMs);
  }

  sendListConnectorTools(
    configKey: string,
    context: { workspaceRoot?: string | null },
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<ListConnectorToolsPayload>> {
    return this.request({ configKey, context, type: "list_connector_tools" }, timeoutMs) as Promise<TypedResponseFrame<ListConnectorToolsPayload>>;
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
    scope: "global" | "workspace";
    trustProject?: boolean;
    context: { workspaceRoot?: string | null };
  }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ ...input, type: "add_connector" }, timeoutMs);
  }

  sendRemoveConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ configKey, context, type: "remove_connector" }, timeoutMs);
  }

  /** This wait ends at acceptance, not at completion of the recovery round. */
  sendReloadConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = this.timeoutMs()): Promise<TypedResponseFrame<ConnectorReloadReceipt>> {
    return this.request({ configKey, context, type: "reload_connector" }, timeoutMs) as Promise<TypedResponseFrame<ConnectorReloadReceipt>>;
  }
  sendGetProjectTrust(path: string, timeoutMs = this.timeoutMs()): Promise<TypedResponseFrame<ProjectTrustPayload>> {
    return this.request({ path, type: "get_project_trust" }, timeoutMs) as Promise<TypedResponseFrame<ProjectTrustPayload>>;
  }

  sendTrustProject(projectRoot: string, timeoutMs = this.timeoutMs()): Promise<TypedResponseFrame<ProjectTrustPayload>> {
    return this.request({ projectRoot, type: "trust_project" }, timeoutMs) as Promise<TypedResponseFrame<ProjectTrustPayload>>;
  }

  sendTestConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ configKey, context, type: "test_connector" }, timeoutMs);
  }

  sendLoginConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = 300_000): Promise<ResponseFrame> {
    return this.request({ configKey, context, type: "login_connector" }, timeoutMs);
  }

  sendLogoutConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ configKey, context, type: "logout_connector" }, timeoutMs);
  }

  sendCancelLoginConnector(configKey: string, context: { workspaceRoot?: string | null }, timeoutMs = this.timeoutMs()): Promise<ResponseFrame> {
    return this.request({ configKey, context, type: "cancel_login_connector" }, timeoutMs);
  }
  sendSetConnectorToolFilter(
    configKey: string,
    include: string[],
    exclude: string[],
    context: { workspaceRoot?: string | null },
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request({ configKey, context, exclude, include, type: "set_connector_tool_filter" }, timeoutMs);
  }

  sendSetConnectorToolEnabled(
    configKey: string,
    rawName: string,
    enabled: boolean,
    context: { workspaceRoot?: string | null },
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<SetConnectorToolEnabledResponse>> {
    return this.request({ configKey, context, enabled, rawName, type: "set_connector_tool_enabled" }, timeoutMs) as Promise<TypedResponseFrame<SetConnectorToolEnabledResponse>>;
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

  sendSetDefaultModel(
    modelId: string,
    options: { onlyIfUnset?: boolean } = {},
    timeoutMs = this.timeoutMs(),
  ): Promise<TypedResponseFrame<SetDefaultModelResponse>> {
    return this.request(
      {
        modelId,
        ...(options.onlyIfUnset ? { onlyIfUnset: true } : {}),
        type: "set_default_model",
      },
      timeoutMs,
    ) as Promise<TypedResponseFrame<SetDefaultModelResponse>>;
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

  sendSetSpeed(
    sessionId: string | null | undefined,
    model: string,
    speed: Speed,
    timeoutMs = this.timeoutMs(),
  ): Promise<ResponseFrame> {
    return this.request({ model, speed, sessionId, type: "set_speed" }, timeoutMs);
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
    const socket = this.active?.connection?.socket;
    if (!socket || socket.destroyed || socket.writableEnded) {
      throw new Error("Tomcat shared connection is not writable");
    }
    socket.write(`${JSON.stringify(command)}\n`, "utf8");
  }

  private shutdown(reason: string): void {
    this.rejectPending(new Error(reason));
    const owned = this.active;
    this.active = undefined;
    owned?.abort.abort(reason);
    for (const abort of this.activeControlRequests.values()) abort.abort(reason);
    this.activeControlRequests.clear();
    this.stdoutBuffer = "";
    this.stderrText = "";
    // Half-close: the server cancels this controller's work, then closes its
    // writer. Keep draining until then; never kill the shared process.
    owned?.connection?.socket.end();
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

  private handleStdoutChunk(chunk: string): void {
    this.stdoutBuffer += chunk;

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
        const parentSignal = this.active?.abort.signal;
        const abortForChildExit = () => abort.abort();
        parentSignal?.addEventListener("abort", abortForChildExit, { once: true });
        void this.runControlHandler(handler, frame, this.connectionGeneration, abort.signal)
          .finally(() => {
            parentSignal?.removeEventListener("abort", abortForChildExit);
            if (this.activeControlRequests.get(frame.requestId) === abort) {
              this.activeControlRequests.delete(frame.requestId);
            }
          });
      }
      return;
    }

    const pending = this.pendingControl.get(frame.requestId);
    if (frame.type === "control_cancel") {
      const active = this.activeControlRequests.get(frame.requestId);
      active?.abort();
      if (!pending) {
        if (!active) this.log("debug", `dropping unknown control cancel ${frame.requestId}`);
        return;
      }
    }

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
      if (signal.aborted || generation !== this.connectionGeneration || !this.active) return;
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
      if (signal.aborted || generation !== this.connectionGeneration || !this.active) return;
      this.emitFrameError(toError(error));
      this.sendControlCancel(frame.requestId, frame.sessionId, null);
    }
  }

  private handleConnectionExit(owned: OwnedConnection, failure?: Error): void {
    if (this.active !== owned) return;
    const error = failure ?? new Error("Tomcat shared connection closed");
    this.active = undefined;
    owned.abort.abort(error);
    this.stdoutBuffer = "";
    this.rejectPending(error);
    if (failure) owned.connection?.socket.destroy();
    const payload: TomcatMessengerExit = { code: null, signal: null, error, stderr: this.stderrText };
    for (const listener of this.exitListeners) listener(payload);
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
