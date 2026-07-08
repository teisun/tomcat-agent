import * as fs from "node:fs/promises";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as vscode from "vscode";

import {
  TOMCAT_ADD_FILE_TO_CHAT_COMMAND,
  TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
  PARTICIPANT_ID,
  TOMCAT_FOCUS_WEBVIEW_COMMAND,
  TEST_DEFAULT_CWD_ENV,
  TEST_EXTRA_ARGS_ENV,
  TEST_INFO_ACTION_ENV,
  TEST_SUPPRESS_EXIT_PROMPT_ENV,
  TEST_WARNING_ACTION_ENV,
  TOMCAT_CONFIG_SECTION,
  TOMCAT_EXECUTABLE_NAME,
  TOMCAT_UI_MODE_SETTING,
  TOMCAT_LIST_SESSIONS_COMMAND,
  TOMCAT_NEW_SESSION_COMMAND,
  TOMCAT_OPEN_SETTINGS_COMMAND,
  TOMCAT_RESTART_COMMAND,
  TOMCAT_WEBVIEW_CONTAINER_ID,
  TOMCAT_WEBVIEW_ID,
} from "./constants";
import {
  resolveTomcatExecutable,
  type ResolvedTomcatExecutable,
} from "./config/resolveTomcatExecutable";
import { VsCodeIde } from "./ide/VsCodeIde";
import {
  hasModelAdminCapabilities,
  initializeServe,
  type InitializeResult,
} from "./serveClient/initialize";
import { SessionRouter } from "./serveClient/sessionRouter";
import { TomcatMessenger } from "./serveClient/TomcatMessenger";
import type { ServeEvent } from "./serveClient/wire";
import { createParticipantHandler } from "./ui/participant/handler";
import {
  ParticipantCommands,
  type PendingQuestionSnapshot,
} from "./ui/participant/commands";
import { SessionOwnershipTracker } from "./ui/webview/ownership";
import {
  createHostFrameMessageId,
  type FrontendOwnerKind,
  type TomcatUiMode,
  type WebviewDomAction,
  type WebviewIntent,
} from "./ui/webview/protocol";
import {
  buildSelectionReference,
  resolveUriToFileReference,
} from "./ui/webview/contextReferences";
import { SettingsPanel } from "./ui/settings/SettingsPanel";
import type { SettingsIntent, SettingsStateSnapshot } from "./shared/settingsProtocol";
import { TomcatWebviewViewProvider } from "./ui/webview/provider";

export type { WebviewIntent } from "./ui/webview/protocol";

let disposeRuntime: (() => void) | undefined;
const execFileAsync = promisify(execFile);
const SETUP_TERMINAL_NAME = "Tomcat Setup";
const START_SETUP_ACTION = "Start Setup";
const RETRY_SETUP_ACTION = "I've Finished Setup";
const OPEN_GUIDE_ACTION = "View Guide";
const OPEN_SETTINGS_ACTION = "Open Settings";
const OPEN_TERMINAL_ACTION = "Open Terminal";

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

type PromptRecord = {
  actions: string[];
  message: string;
  severity: "info" | "warning";
};

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
      composerFooterPlanStatus: string | null;
      composerPlanStatusInBarCount: number;
      composerRowCount: number;
      ctxLabel: string | null;
      disabledTestIds: string[];
      expandedThinkingCount: number;
      expandedToolTitles: string[];
      fileChipTopWithinStream: number | null;
      fileChipVisible: boolean;
      hasConflict: boolean;
      historyLoaderVisible: boolean;
      html: string;
      jumpToLatestVisible: boolean;
      latestUserTopWithinStream: number | null;
      messageTexts: string[];
      overflowAnchor: string | null;
      sessionTabs: string[];
      sessionGroupHeaders: string[];
      sessionMoreButtons: string[];
      stickyPromptText: string | null;
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
      assistantResponseGroups: number;
      groupFoldTitles: string[];
      userPromptPill: boolean;
      assistantNoCard: boolean;
      planCardCount: number;
      planFooterSameRow: boolean;
      planCardTodoCountText: string | null;
      planCardTitleText: string | null;
      planNoticeReplayed: boolean;
      planStateText: string | null;
      progressRow: boolean;
      planTodos: number;
      todoWidgetExpanded: boolean;
      todoWidgetItemCount: number;
      todoWidgetTitle: string | null;
      todoWidgetVisible: boolean;
      toolRowFlat: boolean;
      toolRowExpandable: boolean;
      ellipsisAboveGroupHeader: boolean;
      leftGuideLine: boolean;
      toolRowCount: number;
      toolCardCount: number;
      fileChipOpen: boolean;
      sessionTitleUpdated: boolean;
    }>;
    clearObservedEvents(): void;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    focusWebview(): Promise<void>;
    getObservedEvents(): ServeEvent[];
    getOwnership(): Array<{ owner: FrontendOwnerKind; sessionId: string }>;
    getPendingQuestion(requestId?: string): PendingQuestionSnapshot | undefined;
    getPromptHistory(): PromptRecord[];
    getPreparedChange(toolCallId: string): {
      displayPath: string;
      originalContent: string;
      proposedContent: string;
      toolCallId: string;
    } | undefined;
    getResolvedExecutable(): ResolvedTomcatExecutable;
    getSessionState(sessionId?: string): Promise<Awaited<ReturnType<SessionRouter["getState"]>>>;
    getSettingsPanelState(): {
      route: "models";
      state: SettingsStateSnapshot;
      visible: boolean;
    };
    getWebviewState(): ReturnType<TomcatWebviewViewProvider["currentState"]>;
    injectServeEvent(event: ServeEvent): Promise<void>;
    listSessions(
      scope?: Parameters<SessionRouter["listSessions"]>[0],
    ): Promise<Awaited<ReturnType<SessionRouter["listSessions"]>>>;
    openPreparedDiff(toolCallId: string): Promise<void>;
    releaseSessionOwnership(
      sessionId: string,
      owner?: FrontendOwnerKind,
    ): boolean;
    reloadWebview(): Promise<void>;
    restartServe(): Promise<void>;
    runParticipantTurn(options: RunParticipantTurnOptions): Promise<RunParticipantTurnResult>;
    sendWebviewDomAction(action: WebviewDomAction): Promise<void>;
    sendWebviewIntent(
      intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>,
    ): Promise<void>;
    sendSettingsIntent(intent: SettingsIntent): Promise<void>;
    setOpenDialogHandler(
      handler:
        | ((
            options: vscode.OpenDialogOptions,
          ) => Thenable<readonly vscode.Uri[] | undefined> | readonly vscode.Uri[] | undefined)
        | undefined,
    ): void;
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

const CODELENS_REFRESH_DEBOUNCE_MS = 150;

export class TomcatSelectionCodeLensProvider implements vscode.CodeLensProvider, vscode.Disposable {
  private readonly changeEmitter = new vscode.EventEmitter<void>();

  readonly onDidChangeCodeLenses = this.changeEmitter.event;

  dispose(): void {
    this.changeEmitter.dispose();
  }

  refresh(): void {
    this.changeEmitter.fire();
  }

  provideCodeLenses(document: vscode.TextDocument): vscode.CodeLens[] {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.uri.toString() !== document.uri.toString()) {
      return [];
    }
    const reference = buildSelectionReference(editor);
    if (!reference) {
      return [];
    }
    return [
      new vscode.CodeLens(new vscode.Range(editor.selection.start.line, 0, editor.selection.start.line, 0), {
        command: TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
        title: "Add to Tomcat Chat",
      }),
    ];
  }
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

function autoSelectedPromptAction(
  severity: PromptRecord["severity"],
  actions: readonly string[],
): string | undefined {
  const envName = severity === "info" ? TEST_INFO_ACTION_ENV : TEST_WARNING_ACTION_ENV;
  const configured = process.env[envName]?.trim();
  return configured && actions.includes(configured) ? configured : undefined;
}

async function showPromptMessage(
  promptHistory: PromptRecord[],
  severity: PromptRecord["severity"],
  message: string,
  actions: string[] = [],
): Promise<string | undefined> {
  promptHistory.push({
    actions: [...actions],
    message,
    severity,
  });

  const autoSelected = autoSelectedPromptAction(severity, actions);
  if (autoSelected) {
    return autoSelected;
  }

  if (shouldSuppressExitPrompt()) {
    return undefined;
  }

  return severity === "info"
    ? vscode.window.showInformationMessage(message, ...actions)
    : vscode.window.showWarningMessage(message, ...actions);
}

async function showInformationMessage(
  promptHistory: PromptRecord[],
  message: string,
): Promise<void> {
  await showPromptMessage(promptHistory, "info", message);
}

async function showWarningMessage(
  promptHistory: PromptRecord[],
  message: string,
): Promise<void> {
  await showPromptMessage(promptHistory, "warning", message);
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

function bundledExecutableName(platform: NodeJS.Platform = process.platform): string {
  return platform === "win32" ? "tomcat.exe" : "tomcat";
}

function bundledExecutableCandidate(extensionPath: string): string {
  return path.join(extensionPath, "bin", bundledExecutableName());
}

function quoteForTerminal(command: string): string {
  if (process.platform === "win32") {
    return `"${command.replace(/"/g, '""')}"`;
  }
  return `'${command.replace(/'/g, `'\\''`)}'`;
}

function buildInitCommand(executable: string): string {
  if (executable.includes(path.sep) || executable.includes("\\")) {
    return `${quoteForTerminal(executable)} init`;
  }
  return `${executable} init`;
}

async function clearMacQuarantine(targetPath: string): Promise<void> {
  if (process.platform !== "darwin") {
    return;
  }
  try {
    await execFileAsync("xattr", ["-dr", "com.apple.quarantine", targetPath]);
  } catch {
    // Best-effort only. Browser-downloaded binaries may carry quarantine flags.
  }
}

function isReadonlyExecutableError(error: unknown): boolean {
  if (!error || typeof error !== "object") {
    return false;
  }
  const code = "code" in error ? String((error as { code?: unknown }).code ?? "") : "";
  return code === "EPERM" || code === "EROFS";
}

async function ensureBundledExecutable(
  context: vscode.ExtensionContext,
  candidatePath: string,
): Promise<string> {
  try {
    await fs.access(candidatePath);
  } catch {
    return candidatePath;
  }

  try {
    if (process.platform !== "win32") {
      await fs.chmod(candidatePath, 0o755);
      await clearMacQuarantine(candidatePath);
    }
    return candidatePath;
  } catch (error) {
    if (!isReadonlyExecutableError(error)) {
      return candidatePath;
    }
  }

  const fallbackDir = context.globalStorageUri.fsPath;
  const fallbackPath = path.join(fallbackDir, path.basename(candidatePath));
  await fs.mkdir(fallbackDir, { recursive: true });
  await fs.copyFile(candidatePath, fallbackPath);
  if (process.platform !== "win32") {
    await fs.chmod(fallbackPath, 0o755);
    await clearMacQuarantine(fallbackPath);
  }
  return fallbackPath;
}

async function openExtensionGuide(context: vscode.ExtensionContext): Promise<void> {
  const guidePath = path.join(context.extensionPath, "README.md");
  const document = await vscode.workspace.openTextDocument(guidePath);
  await vscode.window.showTextDocument(document, { preview: true });
}

async function resolveExecutable(
  context: vscode.ExtensionContext,
): Promise<ResolvedTomcatExecutable> {
  const bundledPath = await ensureBundledExecutable(
    context,
    bundledExecutableCandidate(context.extensionPath),
  );
  return resolveTomcatExecutable({
    bundledPath,
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
  const promptHistory: PromptRecord[] = [];
  const eventWaiters = new Set<{
    filter: ObservedEventFilter;
    reject(error: Error): void;
    resolve(event: ServeEvent): void;
    timeout: NodeJS.Timeout;
  }>();
  const recordObservedEvent = (event: ServeEvent) => {
    observedEvents.push(event);
    for (const waiter of [...eventWaiters]) {
      if (!matchesObservedEvent(event, waiter.filter)) {
        continue;
      }
      clearTimeout(waiter.timeout);
      eventWaiters.delete(waiter);
      waiter.resolve(event);
    }
  };
  let resolvedExecutable = await resolveExecutable(context);

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
  let firstRunSetupInProgress = false;
  let firstRunRetryAttemptsRemaining = 0;
  let firstRunRetryTimer: NodeJS.Timeout | undefined;
  let setupTerminal: vscode.Terminal | undefined;

  const clearFirstRunRetryTimer = (): void => {
    if (firstRunRetryTimer) {
      clearTimeout(firstRunRetryTimer);
      firstRunRetryTimer = undefined;
    }
  };

  const openTomcatPathSettings = async (): Promise<void> => {
    await vscode.commands.executeCommand(
      "workbench.action.openSettings",
      `${TOMCAT_CONFIG_SECTION}.path`,
    );
  };

  const ensureSetupTerminal = (): vscode.Terminal => {
    if (!setupTerminal || setupTerminal.exitStatus) {
      setupTerminal = vscode.window.createTerminal({
        cwd: getDefaultCwd(),
        name: SETUP_TERMINAL_NAME,
      });
    }
    return setupTerminal;
  };

  const maybeShowSetupRecoveryMessage = async (
    message: string,
    severity: "info" | "warning" = "info",
  ): Promise<void> => {
    const selection = severity === "warning"
      ? await showPromptMessage(
          promptHistory,
          "warning",
          message,
          [RETRY_SETUP_ACTION, OPEN_TERMINAL_ACTION, OPEN_GUIDE_ACTION],
        )
      : await showPromptMessage(
          promptHistory,
          "info",
          message,
          [RETRY_SETUP_ACTION, OPEN_TERMINAL_ACTION, OPEN_GUIDE_ACTION],
        );

    if (selection === RETRY_SETUP_ACTION) {
      const recovered = await retryInitializationAfterSetup(true);
      if (!recovered) {
        await maybeShowSetupRecoveryMessage(
          "Tomcat is not ready yet. Finish the `tomcat init` prompts in the integrated terminal, then try again.",
          "warning",
        );
      }
      return;
    }
    if (selection === OPEN_TERMINAL_ACTION) {
      ensureSetupTerminal().show(true);
      return;
    }
    if (selection === OPEN_GUIDE_ACTION) {
      void openExtensionGuide(context);
    }
  };

  const stopFirstRunSetup = (): void => {
    firstRunSetupInProgress = false;
    firstRunRetryAttemptsRemaining = 0;
    clearFirstRunRetryTimer();
  };

  const maybeShowExecutableWarning = async (): Promise<void> => {
    if (resolvedExecutable.found) {
      return;
    }

    const selection = await showPromptMessage(
      promptHistory,
      "warning",
      "Tomcat CLI was not found automatically. Install a bundled VSIX for your platform, or install `tomcat` on your PATH, or set tomcat.path if VS Code does not inherit your shell environment.",
      [OPEN_GUIDE_ACTION, OPEN_SETTINGS_ACTION],
    );
    if (selection === OPEN_GUIDE_ACTION) {
      await openExtensionGuide(context);
      return;
    }
    if (selection === OPEN_SETTINGS_ACTION) {
      await openTomcatPathSettings();
    }
  };

  const retryInitializationAfterSetup = async (showSuccessMessage: boolean): Promise<boolean> => {
    try {
      await applyRuntimeConfiguration();
      messenger.restart();
      initializePromise = undefined;
      sessionRouter.clearBootstrapSessionId();
      const result = await ensureInitialized();
      stopFirstRunSetup();
      if (showSuccessMessage) {
        await showInformationMessage(
          promptHistory,
          `Tomcat setup finished. Active session: ${result.sessionId ?? "n/a"}`,
        );
      }
      return true;
    } catch (error) {
      appendOutput(output, "debug", `setup retry still waiting: ${String(error)}`);
      return false;
    }
  };

  const scheduleFirstRunRetryLoop = (): void => {
    clearFirstRunRetryTimer();
    if (!firstRunSetupInProgress) {
      return;
    }

    firstRunRetryAttemptsRemaining = 24;
    const tick = async (): Promise<void> => {
      if (!firstRunSetupInProgress) {
        return;
      }
      if (firstRunRetryAttemptsRemaining <= 0) {
        clearFirstRunRetryTimer();
        void maybeShowSetupRecoveryMessage(
          "Tomcat is still waiting for first-time setup. Finish `tomcat init` in the integrated terminal, then choose `I've Finished Setup` to reconnect.",
          "warning",
        );
        return;
      }

      firstRunRetryAttemptsRemaining -= 1;
      const recovered = await retryInitializationAfterSetup(false);
      if (recovered || !firstRunSetupInProgress) {
        return;
      }
      firstRunRetryTimer = setTimeout(() => {
        void tick();
      }, 5_000);
    };

    firstRunRetryTimer = setTimeout(() => {
      void tick();
    }, 5_000);
  };

  const startFirstRunSetup = async (): Promise<void> => {
    firstRunSetupInProgress = true;
    hasShownInitializationHint = true;
    const terminal = ensureSetupTerminal();
    const initCommand = buildInitCommand(resolvedExecutable.executable);
    terminal.show(true);
    terminal.sendText(initCommand, true);
    appendOutput(output, "info", `started first-run setup: ${initCommand}`);
    scheduleFirstRunRetryLoop();
    await maybeShowSetupRecoveryMessage(
      "Tomcat setup is running in the integrated terminal. Finish the prompts there, then choose `I've Finished Setup` if Tomcat does not reconnect automatically.",
    );
  };

  const maybeShowInitializationHint = async (): Promise<void> => {
    if (hasShownInitializationHint || firstRunSetupInProgress) {
      return;
    }

    hasShownInitializationHint = true;
    const selection = await showPromptMessage(
      promptHistory,
      "info",
      "Tomcat is installed, but it is not ready yet (usually about 1 minute to finish setup): choose a default model, add your API key, and initialize the local runtime.",
      [START_SETUP_ACTION, OPEN_GUIDE_ACTION],
    );
    if (selection === START_SETUP_ACTION) {
      await startFirstRunSetup();
      return;
    }
    if (selection === OPEN_GUIDE_ACTION) {
      await openExtensionGuide(context);
    }
  };

  const applyRuntimeConfiguration = async (): Promise<void> => {
    resolvedExecutable = await resolveExecutable(context);
    if (!firstRunSetupInProgress) {
      hasShownInitializationHint = false;
    }
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

  let testOpenDialogHandler:
    | ((
        options: vscode.OpenDialogOptions,
      ) => Thenable<readonly vscode.Uri[] | undefined> | readonly vscode.Uri[] | undefined)
    | undefined;
  let settingsPanel: SettingsPanel;
  const webviewProvider = new TomcatWebviewViewProvider({
    extensionUri: context.extensionUri,
    getDefaultCwd,
    getUiMode: getTomcatUiMode,
    ide,
    initialize: ensureInitialized,
    messenger,
    openModelSettings: (route) => {
      void ensureInitialized().then((result) => {
        if (hasModelAdminCapabilities(result)) {
          settingsPanel.reveal(route ?? "models");
        }
      });
    },
    ownership,
    sessionRouter,
    showOpenDialog: (options) =>
      testOpenDialogHandler?.(options) ?? vscode.window.showOpenDialog(options),
  });
  settingsPanel = new SettingsPanel({
    ensureInitialized,
    extensionUri: context.extensionUri,
    messenger,
    onModelCatalogChanged: () => webviewProvider.refreshModelCatalog(),
  });
  const selectionCodeLensProvider = new TomcatSelectionCodeLensProvider();
  let selectionCodeLensTimer: ReturnType<typeof setTimeout> | undefined;
  const scheduleSelectionCodeLensRefresh = () => {
    if (selectionCodeLensTimer) {
      clearTimeout(selectionCodeLensTimer);
    }
    selectionCodeLensTimer = setTimeout(() => {
      selectionCodeLensProvider.refresh();
    }, CODELENS_REFRESH_DEBOUNCE_MS);
  };

  const focusWebviewSurface = async (): Promise<string | null> => {
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
    return webviewProvider.currentState().activeSessionId;
  };

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
    recordObservedEvent(event);
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
    void showPromptMessage(
      promptHistory,
      "warning",
      "Tomcat serve exited. Restart the bridge to continue chatting.",
      ["Restart Tomcat"],
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
    participant.iconPath = vscode.Uri.joinPath(context.extensionUri, "media", "icon.png");
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
        promptHistory,
        `Tomcat serve restarted. Active session: ${result.sessionId ?? "n/a"}`,
      );
    },
  );

  const newSessionCommand = vscode.commands.registerCommand(
    TOMCAT_NEW_SESSION_COMMAND,
    async () => {
      await ensureInitialized();
      const sessionId = await sessionRouter.newSession();
      await showInformationMessage(promptHistory, `Created Tomcat session: ${sessionId ?? "unknown"}`);
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
        promptHistory,
        sessionLines.length > 0
          ? `Tomcat sessions: ${sessionLines.join(", ")}`
          : "Tomcat has no active sessions.",
      );
    },
  );
  const focusWebviewCommand = vscode.commands.registerCommand(
    TOMCAT_FOCUS_WEBVIEW_COMMAND,
    async () => {
      await focusWebviewSurface();
    },
  );
  const openSettingsCommand = vscode.commands.registerCommand(
    TOMCAT_OPEN_SETTINGS_COMMAND,
    async (route?: "models") => {
      const initializeResult = await ensureInitialized();
      if (!hasModelAdminCapabilities(initializeResult)) {
        await showWarningMessage(
          promptHistory,
          "The connected `tomcat serve` does not support model management yet.",
        );
        return;
      }
      settingsPanel.reveal(route ?? "models");
    },
  );
  const addSelectionToChatCommand = vscode.commands.registerCommand(
    TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        await showWarningMessage(promptHistory, "Open an editor and select some text first.");
        return;
      }
      const reference = buildSelectionReference(editor);
      if (!reference) {
        await showWarningMessage(promptHistory, "Select some text before adding it to Tomcat Chat.");
        return;
      }
      const sessionId = await focusWebviewSurface();
      if (!sessionId) {
        await showWarningMessage(promptHistory, "Tomcat sidebar is not ready yet. Please try again.");
        return;
      }
      await webviewProvider.postInsertReference(sessionId, reference);
    },
  );
  const addFileToChatCommand = vscode.commands.registerCommand(
    TOMCAT_ADD_FILE_TO_CHAT_COMMAND,
    async (uri?: vscode.Uri, selectedUris?: vscode.Uri[]) => {
      const targets = Array.isArray(selectedUris) && selectedUris.length > 0
        ? selectedUris
        : uri
          ? [uri]
          : [];
      if (!targets.length) {
        await showWarningMessage(promptHistory, "Choose a file or folder in the explorer first.");
        return;
      }
      const sessionId = await focusWebviewSurface();
      if (!sessionId) {
        await showWarningMessage(promptHistory, "Tomcat sidebar is not ready yet. Please try again.");
        return;
      }
      for (const target of targets) {
        const reference = await resolveUriToFileReference(target);
        await webviewProvider.postInsertReference(sessionId, reference);
      }
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
          await showInformationMessage(promptHistory, "Tomcat settings changed. Restarted Tomcat serve.");
        }
        webviewProvider.setUiMode(getTomcatUiMode());
      })().catch((error: unknown) => {
        appendOutput(output, "error", `config update failed: ${String(error)}`);
      });
    },
  );
  const codeLensRegistration = vscode.languages.registerCodeLensProvider(
    [{ scheme: "file" }, { scheme: "vscode-remote" }, { scheme: "untitled" }],
    selectionCodeLensProvider,
  );
  const selectionChangeSubscription = vscode.window.onDidChangeTextEditorSelection(() => {
    scheduleSelectionCodeLensRefresh();
  });
  const activeEditorSubscription = vscode.window.onDidChangeActiveTextEditor(() => {
    scheduleSelectionCodeLensRefresh();
  });

  context.subscriptions.push(
    output,
    ide,
    configurationSubscription,
    restartCommand,
    newSessionCommand,
    listSessionsCommand,
    focusWebviewCommand,
    openSettingsCommand,
    addSelectionToChatCommand,
    addFileToChatCommand,
    settingsPanel,
    webviewProvider,
    webviewRegistration,
    codeLensRegistration,
    selectionChangeSubscription,
    activeEditorSubscription,
    selectionCodeLensProvider,
    {
      dispose() {
        if (selectionCodeLensTimer) {
          clearTimeout(selectionCodeLensTimer);
        }
        clearFirstRunRetryTimer();
        setupTerminal?.dispose();
      },
    },
  );
  scheduleSelectionCodeLensRefresh();

  disposeRuntime = () => {
    clearFirstRunRetryTimer();
    setupTerminal?.dispose();
    participant?.dispose();
    askQuestionHandler.dispose();
    observedEventSubscription.dispose();
    stderrSubscription.dispose();
    frameErrorSubscription.dispose();
    exitSubscription.dispose();
    settingsPanel.dispose();
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
        const dom = await webviewProvider.captureDomSnapshot();
        return {
          ...dom,
          fileChipOpen: webviewProvider.getOpenFileObserved(),
          sessionTitleUpdated: observedEvents.some(
            (event) => (event as { type: string }).type === "session.title_updated",
          ),
        };
      },
      clearObservedEvents: () => {
        observedEvents.length = 0;
        webviewProvider.resetOpenFileObserved();
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
      getPromptHistory: () => [...promptHistory],
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
      getSettingsPanelState: () => settingsPanel.__testingSnapshot(),
      getWebviewState: () => webviewProvider.currentState(),
      injectServeEvent: async (event) => {
        recordObservedEvent(event);
        await (
          webviewProvider as unknown as {
            handleServeEvent(frame: ServeEvent): Promise<void>;
          }
        ).handleServeEvent(event);
      },
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
      reloadWebview: async () => {
        webviewProvider.resetForTestReload();
        await webviewProvider.dispatchTestIntent({
          messageId: createHostFrameMessageId("webview-ready"),
          type: "ready",
        });
      },
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
      sendSettingsIntent: async (intent) => {
        await settingsPanel.__testingDispatchIntent(intent);
      },
      sendWebviewDomAction: async (action) => {
        await webviewProvider.dispatchTestDomAction(action);
      },
      setOpenDialogHandler: (handler) => {
        testOpenDialogHandler = handler;
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
