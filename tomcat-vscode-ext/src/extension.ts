import * as vscode from "vscode";

import { VsCodeIde } from "./ide/VsCodeIde";
import { initializeServe, type InitializeResult } from "./serveClient/initialize";
import { SessionRouter } from "./serveClient/sessionRouter";
import { TomcatMessenger } from "./serveClient/TomcatMessenger";
import { createParticipantHandler } from "./ui/participant/handler";
import { ParticipantCommands } from "./ui/participant/commands";

const PARTICIPANT_ID = "tomcat.tomcat";
const TEST_PATH_ENV = "TOMCAT_VSCODE_TEST_PATH";
const TEST_DEFAULT_CWD_ENV = "TOMCAT_VSCODE_TEST_DEFAULT_CWD";
const TEST_EXTRA_ARGS_ENV = "TOMCAT_VSCODE_TEST_EXTRA_ARGS";
const TEST_SUPPRESS_EXIT_PROMPT_ENV = "TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT";

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

export interface TomcatExtensionApi {
  __testing: {
    applyPreparedEdit(toolCallId: string): Promise<boolean>;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    getPreparedChange(toolCallId: string): {
      displayPath: string;
      originalContent: string;
      proposedContent: string;
      toolCallId: string;
    } | undefined;
    getSessionState(sessionId?: string): Promise<Awaited<ReturnType<SessionRouter["getState"]>>>;
    listSessions(): Promise<Awaited<ReturnType<SessionRouter["listSessions"]>>>;
    openPreparedDiff(toolCallId: string): Promise<void>;
    restartServe(): Promise<void>;
    runParticipantTurn(options: RunParticipantTurnOptions): Promise<RunParticipantTurnResult>;
  };
}

function getEnvOverride(name: string): string | undefined {
  const value = process.env[name];
  return value && value.trim() ? value.trim() : undefined;
}

function getTomcatExecutable(): string {
  return getEnvOverride(TEST_PATH_ENV) ?? getTomcatConfiguration().get<string>("path", "tomcat");
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

function getTomcatConfiguration(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration("tomcat");
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

  const messenger = new TomcatMessenger({
    cwd: getDefaultCwd(),
    executable: getTomcatExecutable(),
    extraArgs: getTomcatExtraArgs(),
    logger: {
      debug: (message) => appendOutput(output, "debug", message),
      error: (message) => appendOutput(output, "error", message),
      info: (message) => appendOutput(output, "info", message),
      warn: (message) => appendOutput(output, "warn", message),
    },
  });
  const sessionRouter = new SessionRouter(messenger, getDefaultCwd);

  let initializePromise: Promise<InitializeResult> | undefined;

  const ensureInitialized = async (): Promise<InitializeResult> => {
    if (initializePromise) {
      return initializePromise;
    }

    initializePromise = (async () => {
      messenger.start();
      const result = await initializeServe(messenger);
      if (result.sessionId) {
        sessionRouter.setBootstrapSessionId(result.sessionId);
      }
      return result;
    })();

    try {
      return await initializePromise;
    } catch (error) {
      initializePromise = undefined;
      throw error;
    }
  };

  const askQuestionHandler = messenger.registerAskQuestionHandler(
    async (request, frame) => commands.askUser(request, frame.sessionId),
  );
  const stderrSubscription = messenger.onStderr((chunk) => {
    appendOutput(output, "stderr", chunk);
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
    if (shouldSuppressExitPrompt()) {
      return;
    }
    void vscode.window
      .showWarningMessage(
        "Tomcat serve exited. Restart the bridge to continue chatting.",
        "Restart Tomcat",
      )
      .then((selection) => {
        if (selection === "Restart Tomcat") {
          void vscode.commands.executeCommand("tomcat.restartServe");
        }
      });
  });

  const participantHandler = createParticipantHandler({
    commands,
    ide,
    initialize: ensureInitialized,
    messenger,
    sessionRouter,
  });
  const participant = vscode.chat.createChatParticipant(
    PARTICIPANT_ID,
    participantHandler,
  );
  participant.iconPath = new vscode.ThemeIcon("terminal");

  const restartCommand = vscode.commands.registerCommand(
    "tomcat.restartServe",
    async () => {
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
    "tomcat.session.new",
    async () => {
      await ensureInitialized();
      const sessionId = await sessionRouter.newSession();
      await showInformationMessage(`Created Tomcat session: ${sessionId ?? "unknown"}`);
    },
  );

  const listSessionsCommand = vscode.commands.registerCommand(
    "tomcat.session.list",
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

  context.subscriptions.push(
    output,
    ide,
    participant,
    restartCommand,
    newSessionCommand,
    listSessionsCommand,
  );

  disposeRuntime = () => {
    askQuestionHandler.dispose();
    stderrSubscription.dispose();
    frameErrorSubscription.dispose();
    exitSubscription.dispose();
    messenger.dispose();
    ide.dispose();
  };

  const api: TomcatExtensionApi = {
    __testing: {
      applyPreparedEdit: (toolCallId) => ide.applyPreparedEdit(toolCallId),
      executeCommand: (command, ...args) =>
        vscode.commands.executeCommand(command, ...args),
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
      getSessionState: async (sessionId?: string) => {
        await ensureInitialized();
        return sessionRouter.getState(sessionId);
      },
      listSessions: async () => {
        await ensureInitialized();
        return sessionRouter.listSessions();
      },
      openPreparedDiff: (toolCallId) => ide.openPreparedDiff(toolCallId),
      restartServe: async () => {
        await vscode.commands.executeCommand("tomcat.restartServe");
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
    },
  };

  return api;
}

export async function deactivate(): Promise<void> {
  disposeRuntime?.();
  disposeRuntime = undefined;
}
