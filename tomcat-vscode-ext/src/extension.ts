import * as vscode from "vscode";

import {
  PARTICIPANT_ID,
  TOMCAT_FOCUS_WEBVIEW_COMMAND,
  TEST_DEFAULT_CWD_ENV,
  TEST_EXTRA_ARGS_ENV,
  TEST_SUPPRESS_EXIT_PROMPT_ENV,
  TOMCAT_CONFIG_SECTION,
  TOMCAT_EXECUTABLE_NAME,
  TOMCAT_UI_MODE_SETTING,
  TOMCAT_LIST_SESSIONS_COMMAND,
  TOMCAT_NEW_SESSION_COMMAND,
  TOMCAT_RESTART_COMMAND,
  TOMCAT_WEBVIEW_CONTAINER_ID,
  TOMCAT_WEBVIEW_ID,
} from "./constants";
import {
  resolveTomcatExecutable,
  type ResolvedTomcatExecutable,
} from "./config/resolveTomcatExecutable";
import { VsCodeIde } from "./ide/VsCodeIde";
import { initializeServe, type InitializeResult } from "./serveClient/initialize";
import { SessionRouter } from "./serveClient/sessionRouter";
import { TomcatMessenger } from "./serveClient/TomcatMessenger";
import type { ServeEvent } from "./serveClient/wire";
import { createParticipantHandler } from "./ui/participant/handler";
import {
  ParticipantCommands,
  type PendingQuestionSnapshot,
} from "./ui/participant/commands";
import { SessionOwnershipTracker } from "./ui/webview/ownership";
import type {
  FrontendOwnerKind,
  TomcatUiMode,
  WebviewDomAction,
  WebviewIntent,
} from "./ui/webview/protocol";
import { TomcatWebviewViewProvider } from "./ui/webview/provider";

export type { WebviewIntent } from "./ui/webview/protocol";

let disposeRuntime: (() => void) | undefined;

type CapturedStreamEvent =
  | {
      kind: "anchor";
      label: string;
      uri: string;
    }
  | {
      arguments?: unknown[];
      command: string;
      kind: "button";
      title: string;
    }
  | {
      kind: "markdown" | "progress";
      value: string;
    };

export interface RunParticipantTurnOptions {
  autoClickTitles?: string[];
  cancelAfterMs?: number;
  historySessionId?: string;
  prompt: string;
}

export interface RunParticipantTurnResult {
  result: vscode.ChatResult | undefined;
  stream: CapturedStreamEvent[];
}

export interface ObservedEventFilter {
  sessionId?: string;
  textIncludes?: string;
  timeoutMs?: number;
  type?: ServeEvent["type"];
}

export interface TomcatExtensionApi {
  __testing: {
    applyPreparedEdit(toolCallId: string): Promise<boolean>;
    captureWebviewDom(): Promise<{
      activeSessionId: string | null;
      approvalCount: number;
      composerControlMetrics: Record<
        string,
        {
          top: number;
          width: number;
        }
      >;
      composerRowCount: number;
      expandedThinkingCount: number;
      expandedToolTitles: string[];
      hasConflict: boolean;
      html: string;
      jumpToLatestVisible: boolean;
      messageTexts: string[];
      sessionTabs: string[];
      streamMetrics: {
        clientHeight: number;
        distanceFromBottom: number;
        scrollHeight: number;
        scrollTop: number;
      };
      timelineKinds: string[];
      toolBodyMetrics: Array<{
        clientHeight: number;
        expanded: boolean;
        scrollHeight: number;
        title: string;
      }>;
      toolTitles: string[];
    }>;
    clearObservedEvents(): void;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    focusWebview(): Promise<void>;
    getObservedEvents(): ServeEvent[];
    getOwnership(): Array<{ owner: FrontendOwnerKind; sessionId: string }>;
    getPendingQuestion(requestId?: string): PendingQuestionSnapshot | undefined;
    getPreparedChange(toolCallId: string): {
      displayPath: string;
      originalContent: string;
      proposedContent: string;
      toolCallId: string;
    } | undefined;
    getResolvedExecutable(): ResolvedTomcatExecutable;
    getSessionState(sessionId?: string): Promise<Awaited<ReturnType<SessionRouter["getState"]>>>;
    getWebviewState(): ReturnType<TomcatWebviewViewProvider["currentState"]>;
    listSessions(
      scope?: Parameters<SessionRouter["listSessions"]>[0],
    ): Promise<Awaited<ReturnType<SessionRouter["listSessions"]>>>;
    openPreparedDiff(toolCallId: string): Promise<void>;
    releaseSessionOwnership(
      sessionId: string,
      owner?: FrontendOwnerKind,
    ): boolean;
    restartServe(): Promise<void>;
    runParticipantTurn(options: RunParticipantTurnOptions): Promise<RunParticipantTurnResult>;
    sendWebviewDomAction(action: WebviewDomAction): Promise<void>;
    sendWebviewIntent(
      intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>,
    ): Promise<void>;
    setParticipantUiOverrides(
      overrides: Parameters<ParticipantCommands["setUiOverrides"]>[0],
    ): void;
    waitForEvent(filter: ObservedEventFilter): Promise<ServeEvent>;
    waitForPendingQuestion(timeoutMs?: number): Promise<PendingQuestionSnapshot>;
    waitForWebviewReady(timeoutMs?: number): Promise<void>;
  };
}

function getEnvOverride(name: string): string | undefined {
  const value = process.env[name];
  return value && value.trim() ? value.trim() : undefined;
}

function matchesObservedEvent(
  event: ServeEvent,
  filter: ObservedEventFilter,
): boolean {
  if (filter.type && event.type !== filter.type) {
    return false;
  }
  if (filter.sessionId && event.sessionId !== filter.sessionId) {
    return false;
  }
  if (filter.textIncludes && !JSON.stringify(event).includes(filter.textIncludes)) {
    return false;
  }
  return true;
}

function getTomcatConfiguration(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration(TOMCAT_CONFIG_SECTION);
}

function getTomcatUiMode(): TomcatUiMode {
  const configured = getTomcatConfiguration().get<TomcatUiMode>(
    TOMCAT_UI_MODE_SETTING,
    "both",
  );
  return configured === "participant" || configured === "webview"
    ? configured
    : "both";
}

function isTomcatPathConfigured(): boolean {
  const inspect = getTomcatConfiguration().inspect<string>("path");
  return (
    inspect?.globalValue !== undefined ||
    inspect?.workspaceFolderValue !== undefined ||
    inspect?.workspaceValue !== undefined
  );
}

function getTomcatExtraArgs(): string[] {
  const envValue = getEnvOverride(TEST_EXTRA_ARGS_ENV);
  if (!envValue) {
    return getTomcatConfiguration().get<string[]>("serve.extraArgs", []);
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(envValue);
  } catch (error) {
    throw new Error(
      `Invalid ${TEST_EXTRA_ARGS_ENV}: ${String(error)}`,
    );
  }

  if (!Array.isArray(parsed) || !parsed.every((entry) => typeof entry === "string")) {
    throw new Error(`${TEST_EXTRA_ARGS_ENV} must be a JSON string array`);
  }

  return parsed;
}

function shouldSuppressExitPrompt(): boolean {
  return process.env[TEST_SUPPRESS_EXIT_PROMPT_ENV] === "1";
}

async function showInformationMessage(message: string): Promise<void> {
  if (shouldSuppressExitPrompt()) {
    return;
  }

  await vscode.window.showInformationMessage(message);
}

function getDefaultCwd(): string | undefined {
  const envOverride = getEnvOverride(TEST_DEFAULT_CWD_ENV);
  if (envOverride) {
    return envOverride;
  }

  const configured = getTomcatConfiguration().get<string>("session.defaultCwd");
  if (configured && configured.trim()) {
    return configured.trim();
  }

  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function resolveExecutable(): Promise<ResolvedTomcatExecutable> {
  return resolveTomcatExecutable({
    configuredPath: getTomcatConfiguration().get<string>("path", TOMCAT_EXECUTABLE_NAME),
    pathWasConfigured: isTomcatPathConfigured(),
  });
}

function appendOutput(
  output: vscode.OutputChannel,
  prefix: string,
  message: string,
): void {
  for (const line of message.split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    output.appendLine(`[${prefix}] ${line}`);
  }
}

export async function activate(
  context: vscode.ExtensionContext,
): Promise<TomcatExtensionApi> {
  const output = vscode.window.createOutputChannel("Tomcat");
  const ide = new VsCodeIde();
  const commands = new ParticipantCommands(ide);
  commands.register(context);
  const observedEvents: ServeEvent[] = [];
  const eventWaiters = new Set<{
    filter: ObservedEventFilter;
    reject(error: Error): void;
    resolve(event: ServeEvent): void;
    timeout: NodeJS.Timeout;
  }>();
  let resolvedExecutable = await resolveExecutable();

  const messenger = new TomcatMessenger({
    cwd: getDefaultCwd(),
    executable: resolvedExecutable.executable,
    extraArgs: getTomcatExtraArgs(),
    logger: {
      debug: (message) => appendOutput(output, "debug", message),
      error: (message) => appendOutput(output, "error", message),
      info: (message) => appendOutput(output, "info", message),
      warn: (message) => appendOutput(output, "warn", message),
    },
  });
  const sessionRouter = new SessionRouter(messenger, getDefaultCwd);
  const ownership = new SessionOwnershipTracker();

  let initializePromise: Promise<InitializeResult> | undefined;
  let hasShownInitializationHint = false;

  const maybeShowExecutableWarning = async (): Promise<void> => {
    if (resolvedExecutable.found || shouldSuppressExitPrompt()) {
      return;
    }

    const selection = await vscode.window.showWarningMessage(
      "Tomcat CLI was not found automatically. Install `tomcat` on your PATH or set tomcat.path if VS Code does not inherit your shell environment.",
      "Open Settings",
    );
    if (selection === "Open Settings") {
      await vscode.commands.executeCommand(
        "workbench.action.openSettings",
        `${TOMCAT_CONFIG_SECTION}.path`,
      );
    }
  };

  const maybeShowInitializationHint = async (): Promise<void> => {
    if (hasShownInitializationHint || shouldSuppressExitPrompt()) {
      return;
    }

    hasShownInitializationHint = true;
    await vscode.window.showWarningMessage(
      "Tomcat was found, but the runtime could not initialize. If this is your first time using Tomcat, run `tomcat init` once in a terminal and try again.",
    );
  };

  const applyRuntimeConfiguration = async (): Promise<void> => {
    resolvedExecutable = await resolveExecutable();
    hasShownInitializationHint = false;
    messenger.updateOptions({
      cwd: getDefaultCwd(),
      executable: resolvedExecutable.executable,
      extraArgs: getTomcatExtraArgs(),
    });
    appendOutput(
      output,
      "info",
      `tomcat executable: ${resolvedExecutable.executable} (${resolvedExecutable.source})`,
    );
    void maybeShowExecutableWarning();
  };

  await applyRuntimeConfiguration();

  const ensureInitialized = async (): Promise<InitializeResult> => {
    if (initializePromise) {
      return initializePromise;
    }

    initializePromise = (async () => {
      messenger.start();
      const result = await initializeServe(messenger);
      hasShownInitializationHint = false;
      if (result.sessionId) {
        sessionRouter.setBootstrapSessionId(result.sessionId);
      }
      return result;
    })();

    try {
      return await initializePromise;
    } catch (error) {
      initializePromise = undefined;
      appendOutput(output, "error", `initialize failed: ${String(error)}`);
      if (resolvedExecutable.found) {
        void maybeShowInitializationHint();
      } else {
        void maybeShowExecutableWarning();
      }
      throw error;
    }
  };

  const webviewProvider = new TomcatWebviewViewProvider({
    extensionUri: context.extensionUri,
    getDefaultCwd,
    getUiMode: getTomcatUiMode,
    ide,
    initialize: ensureInitialized,
    messenger,
    ownership,
    sessionRouter,
  });

  const askQuestionHandler = messenger.registerAskQuestionHandler(
    async (request, frame) => {
      const owner = frame.sessionId
        ? ownership.ownerOf(frame.sessionId)?.owner
        : undefined;
      if (owner === "webview") {
        return webviewProvider.askUser(request, frame.sessionId);
      }
      return commands.askUser(request, frame.sessionId);
    },
  );
  const stderrSubscription = messenger.onStderr((chunk) => {
    appendOutput(output, "stderr", chunk);
  });
  const observedEventSubscription = messenger.onEvent((event) => {
    observedEvents.push(event);
    for (const waiter of [...eventWaiters]) {
      if (!matchesObservedEvent(event, waiter.filter)) {
        continue;
      }
      clearTimeout(waiter.timeout);
      eventWaiters.delete(waiter);
      waiter.resolve(event);
    }
  });
  const frameErrorSubscription = messenger.onFrameError((error) => {
    appendOutput(output, "frame", error.message);
  });
  const exitSubscription = messenger.onExit((event) => {
    initializePromise = undefined;
    sessionRouter.clearBootstrapSessionId();
    appendOutput(
      output,
      "exit",
      `code=${String(event.code)} signal=${String(event.signal)} stderr=${event.stderr.trim()}`,
    );
    if (event.error) {
      appendOutput(output, "error", event.error.message);
    }
    if (shouldSuppressExitPrompt()) {
      return;
    }
    if (!resolvedExecutable.found || event.error?.message.includes("ENOENT")) {
      void maybeShowExecutableWarning();
      return;
    }
    void vscode.window
      .showWarningMessage(
        "Tomcat serve exited. Restart the bridge to continue chatting.",
        "Restart Tomcat",
      )
      .then((selection) => {
        if (selection === "Restart Tomcat") {
          void vscode.commands.executeCommand(TOMCAT_RESTART_COMMAND);
        }
      });
  });

  const participantHandler = createParticipantHandler({
    commands,
    getUiMode: getTomcatUiMode,
    ide,
    initialize: ensureInitialized,
    messenger,
    ownership,
    sessionRouter,
  });
  let participant: vscode.ChatParticipant | undefined;
  const syncParticipantRegistration = (): void => {
    if (getTomcatUiMode() === "webview") {
      participant?.dispose();
      participant = undefined;
      return;
    }
    if (participant) {
      return;
    }
    participant = vscode.chat.createChatParticipant(
      PARTICIPANT_ID,
      participantHandler,
    );
    participant.iconPath = new vscode.ThemeIcon("terminal");
  };
  syncParticipantRegistration();

  const restartCommand = vscode.commands.registerCommand(
    TOMCAT_RESTART_COMMAND,
    async () => {
      await applyRuntimeConfiguration();
      messenger.restart();
      initializePromise = undefined;
      sessionRouter.clearBootstrapSessionId();
      const result = await ensureInitialized();
      await showInformationMessage(
        `Tomcat serve restarted. Active session: ${result.sessionId ?? "n/a"}`,
      );
    },
  );

  const newSessionCommand = vscode.commands.registerCommand(
    TOMCAT_NEW_SESSION_COMMAND,
    async () => {
      await ensureInitialized();
      const sessionId = await sessionRouter.newSession();
      await showInformationMessage(`Created Tomcat session: ${sessionId ?? "unknown"}`);
    },
  );

  const listSessionsCommand = vscode.commands.registerCommand(
    TOMCAT_LIST_SESSIONS_COMMAND,
    async () => {
      await ensureInitialized();
      const payload = await sessionRouter.listSessions();
      const sessionLines = payload.sessions.map((session) => {
        const busy = session.busy ? "busy" : "idle";
        const active =
          payload.activeSessionId === session.sessionId ? " (active)" : "";
        return `${session.sessionId} - ${busy}${active}`;
      });
      await showInformationMessage(
        sessionLines.length > 0
          ? `Tomcat sessions: ${sessionLines.join(", ")}`
          : "Tomcat has no active sessions.",
      );
    },
  );
  const focusWebviewCommand = vscode.commands.registerCommand(
    TOMCAT_FOCUS_WEBVIEW_COMMAND,
    async () => {
      await vscode.commands.executeCommand(
        `workbench.view.extension.${TOMCAT_WEBVIEW_CONTAINER_ID}`,
      );
      try {
        await vscode.commands.executeCommand(`${TOMCAT_WEBVIEW_ID}.focus`);
      } catch {
        // Some host builds do not expose an auto-generated focus command for custom views.
      }
      webviewProvider.reveal();
      await webviewProvider.waitUntilReady().catch(() => undefined);
    },
  );
  const webviewRegistration = vscode.window.registerWebviewViewProvider(
    TOMCAT_WEBVIEW_ID,
    webviewProvider,
    {
      webviewOptions: {
        retainContextWhenHidden: true,
      },
    },
  );
  const configurationSubscription = vscode.workspace.onDidChangeConfiguration(
    (event) => {
      if (
        !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.path`) &&
        !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.serve.extraArgs`) &&
        !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.session.defaultCwd`) &&
        !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.${TOMCAT_UI_MODE_SETTING}`)
      ) {
        return;
      }

      void (async () => {
        const uiModeChanged = event.affectsConfiguration(
          `${TOMCAT_CONFIG_SECTION}.${TOMCAT_UI_MODE_SETTING}`,
        );
        if (uiModeChanged) {
          webviewProvider.setUiMode(getTomcatUiMode());
          syncParticipantRegistration();
        }
        if (
          !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.path`) &&
          !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.serve.extraArgs`) &&
          !event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.session.defaultCwd`)
        ) {
          return;
        }
        await applyRuntimeConfiguration();
        initializePromise = undefined;
        sessionRouter.clearBootstrapSessionId();
        if (messenger.isRunning) {
          messenger.restart();
          await ensureInitialized();
          await showInformationMessage("Tomcat settings changed. Restarted Tomcat serve.");
        }
        webviewProvider.setUiMode(getTomcatUiMode());
      })().catch((error: unknown) => {
        appendOutput(output, "error", `config update failed: ${String(error)}`);
      });
    },
  );

  context.subscriptions.push(
    output,
    ide,
    configurationSubscription,
    restartCommand,
    newSessionCommand,
    listSessionsCommand,
    focusWebviewCommand,
    webviewProvider,
    webviewRegistration,
  );

  disposeRuntime = () => {
    participant?.dispose();
    askQuestionHandler.dispose();
    observedEventSubscription.dispose();
    stderrSubscription.dispose();
    frameErrorSubscription.dispose();
    exitSubscription.dispose();
    for (const waiter of [...eventWaiters]) {
      clearTimeout(waiter.timeout);
      waiter.reject(new Error("Tomcat extension is shutting down"));
      eventWaiters.delete(waiter);
    }
    messenger.dispose();
    webviewProvider.dispose();
    ide.dispose();
  };

  const api: TomcatExtensionApi = {
    __testing: {
      applyPreparedEdit: (toolCallId) => ide.applyPreparedEdit(toolCallId),
      captureWebviewDom: async () => {
        await webviewProvider.waitUntilReady();
        return webviewProvider.captureDomSnapshot();
      },
      clearObservedEvents: () => {
        observedEvents.length = 0;
      },
      executeCommand: (command, ...args) =>
        vscode.commands.executeCommand(command, ...args),
      focusWebview: async () => {
        await vscode.commands.executeCommand(TOMCAT_FOCUS_WEBVIEW_COMMAND);
      },
      getObservedEvents: () => [...observedEvents],
      getOwnership: () =>
        [...ownership.snapshot().entries()].map(([sessionId, owner]) => ({
          owner,
          sessionId,
        })),
      getPendingQuestion: (requestId?: string) => commands.getPendingQuestion(requestId),
      getPreparedChange: (toolCallId) => {
        const change = ide.getPreparedChange(toolCallId);
        if (!change) {
          return undefined;
        }
        return {
          displayPath: change.displayPath,
          originalContent: change.originalContent,
          proposedContent: change.proposedContent,
          toolCallId: change.toolCallId,
        };
      },
      getResolvedExecutable: () => resolvedExecutable,
      getWebviewState: () => webviewProvider.currentState(),
      getSessionState: async (sessionId?: string) => {
        await ensureInitialized();
        return sessionRouter.getState(sessionId);
      },
      listSessions: async (scope = "live") => {
        await ensureInitialized();
        return sessionRouter.listSessions(scope);
      },
      openPreparedDiff: (toolCallId) => ide.openPreparedDiff(toolCallId),
      releaseSessionOwnership: (sessionId, owner) =>
        ownership.release(sessionId, owner),
      restartServe: async () => {
        await vscode.commands.executeCommand(TOMCAT_RESTART_COMMAND);
      },
      runParticipantTurn: async (
        options: RunParticipantTurnOptions,
      ): Promise<RunParticipantTurnResult> => {
        const stream: CapturedStreamEvent[] = [];
        const autoClickTitles = new Set(options.autoClickTitles ?? []);
        const tokenSource = new vscode.CancellationTokenSource();
        const history = options.historySessionId
          ? ([
              {
                participant: PARTICIPANT_ID,
                result: {
                  metadata: {
                    sessionId: options.historySessionId,
                  },
                },
              },
            ] as unknown as readonly (
              | vscode.ChatRequestTurn
              | vscode.ChatResponseTurn
            )[])
          : [];

        const streamCapture = {
          anchor(uri: vscode.Uri, label: string) {
            stream.push({
              kind: "anchor",
              label,
              uri: uri.toString(),
            });
          },
          button(payload: {
            arguments?: unknown[];
            command: string;
            title: string;
          }) {
            stream.push({
              arguments: payload.arguments,
              command: payload.command,
              kind: "button",
              title: payload.title,
            });

            if (!autoClickTitles.has(payload.title)) {
              return;
            }
            autoClickTitles.delete(payload.title);
            queueMicrotask(() => {
              void vscode.commands.executeCommand(
                payload.command,
                ...(payload.arguments ?? []),
              );
            });
          },
          markdown(value: string) {
            stream.push({
              kind: "markdown",
              value,
            });
          },
          progress(value: string) {
            stream.push({
              kind: "progress",
              value,
            });
          },
        } as vscode.ChatResponseStream;

        const cancelTimer =
          typeof options.cancelAfterMs === "number"
            ? setTimeout(() => tokenSource.cancel(), options.cancelAfterMs)
            : undefined;

        try {
          const result = await participantHandler(
            {
              prompt: options.prompt,
            } as vscode.ChatRequest,
            {
              history,
            } as vscode.ChatContext,
            streamCapture,
            tokenSource.token,
          );
          await new Promise((resolve) => setTimeout(resolve, 0));
          return { result: result ?? undefined, stream };
        } finally {
          if (cancelTimer) {
            clearTimeout(cancelTimer);
          }
          tokenSource.dispose();
        }
      },
      sendWebviewIntent: async (intent) => {
        await webviewProvider.dispatchTestIntent(intent);
      },
      sendWebviewDomAction: async (action) => {
        await webviewProvider.dispatchTestDomAction(action);
      },
      setParticipantUiOverrides: (overrides) => {
        commands.setUiOverrides(overrides);
      },
      waitForEvent: async (filter: ObservedEventFilter): Promise<ServeEvent> => {
        const existing = observedEvents.find((event) => matchesObservedEvent(event, filter));
        if (existing) {
          return existing;
        }

        return new Promise<ServeEvent>((resolve, reject) => {
          const timeout = setTimeout(() => {
            eventWaiters.delete(waiter);
            reject(
              new Error(
                `Timed out waiting for Tomcat event ${JSON.stringify(filter)}`,
              ),
            );
          }, filter.timeoutMs ?? 10_000);
          const waiter = {
            filter,
            reject,
            resolve,
            timeout,
          };
          eventWaiters.add(waiter);
        });
      },
      waitForWebviewReady: async (timeoutMs = 15_000) => {
        await webviewProvider.waitUntilReady(timeoutMs);
      },
      waitForPendingQuestion: async (
        timeoutMs = 10_000,
      ): Promise<PendingQuestionSnapshot> => {
        const existing = commands.getPendingQuestion();
        if (existing) {
          return existing;
        }

        return new Promise<PendingQuestionSnapshot>((resolve, reject) => {
          const timeout = setTimeout(() => {
            subscription.dispose();
            reject(new Error("Timed out waiting for a Tomcat approval prompt"));
          }, timeoutMs);
          const subscription = commands.onPendingQuestion((question) => {
            clearTimeout(timeout);
            subscription.dispose();
            resolve(question);
          });
        });
      },
    },
  };

  return api;
}

export async function deactivate(): Promise<void> {
  disposeRuntime?.();
  disposeRuntime = undefined;
}
