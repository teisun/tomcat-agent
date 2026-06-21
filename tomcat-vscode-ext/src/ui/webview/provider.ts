import * as fs from "node:fs";
import * as path from "node:path";

import * as vscode from "vscode";

import type { VsCodeIde } from "../../ide/VsCodeIde";
import {
  hasServeCapability,
  SERVE_CAPABILITY_LIST_MODELS,
  type InitializeResult,
} from "../../serveClient/initialize";
import {
  normalizeAskQuestionResponse,
  type AskQuestionWireRequest,
  type AskQuestionWireResponse,
} from "../../serveClient/protocol";
import type { SessionRouter } from "../../serveClient/sessionRouter";
import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import type { ServeEvent } from "../../serveClient/wire";
import {
  createHostFrameMessageId,
  isWebviewIntent,
  PendingMessageTracker,
  type FrontendOwnerKind,
  type HostEventFrameContent,
  type HostToWebviewFrame,
  type TomcatUiMode,
  type WebviewIntent,
} from "./protocol";
import { SessionOwnershipTracker } from "./ownership";
import { TomcatSessionPool } from "./sessionPool";
import { WebviewStateStore } from "./state";

type PendingQuestion = {
  request: AskQuestionWireRequest;
  resolve(response: AskQuestionWireResponse): void;
  sessionId?: string | null;
};

type DomSnapshot = Extract<
  WebviewIntent,
  { type: "__test.dom_snapshot" }
>["data"];

export interface TomcatWebviewProviderDeps {
  extensionUri: vscode.Uri;
  getDefaultCwd(): string | undefined;
  getUiMode(): TomcatUiMode;
  ide: VsCodeIde;
  initialize(): Promise<InitializeResult>;
  messenger: TomcatMessenger;
  ownership: SessionOwnershipTracker;
  sessionRouter: SessionRouter;
}

function getNonce(): string {
  return Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
}

function isMutationTool(toolName: string): boolean {
  return toolName === "edit" || toolName === "hashline_edit" || toolName === "write";
}

function parseModelIds(payload: unknown): string[] {
  if (typeof payload !== "object" || payload === null) {
    return [];
  }
  const models = (payload as { models?: unknown }).models;
  if (!Array.isArray(models)) {
    return [];
  }
  return models
    .filter(
      (entry): entry is { id: string } =>
        typeof entry === "object" && entry !== null && typeof (entry as { id?: unknown }).id === "string",
    )
    .map((entry) => entry.id);
}

export class TomcatWebviewViewProvider implements vscode.WebviewViewProvider, vscode.Disposable {
  private readonly domSnapshots = new PendingMessageTracker<DomSnapshot>();
  private readonly pendingQuestions = new Map<string, PendingQuestion>();
  private readonly readyWaiters = new Set<{
    reject(error: Error): void;
    resolve(): void;
    timeout: NodeJS.Timeout;
  }>();
  private readonly sessionPool: TomcatSessionPool;
  private readonly stateStore: WebviewStateStore;
  private readonly eventSubscription: { dispose(): void };
  private initialized?: InitializeResult;
  private isReady = false;
  private messageSubscription?: vscode.Disposable;
  private uiMode: TomcatUiMode;
  private view?: vscode.WebviewView;

  constructor(private readonly deps: TomcatWebviewProviderDeps) {
    this.sessionPool = new TomcatSessionPool(deps.sessionRouter);
    this.uiMode = deps.getUiMode();
    this.stateStore = new WebviewStateStore(this.uiMode);
    this.eventSubscription = deps.messenger.onEvent((event) => {
      void this.handleServeEvent(event);
    });
  }

  dispose(): void {
    this.messageSubscription?.dispose();
    this.eventSubscription.dispose();
    this.domSnapshots.rejectAll(new Error("Tomcat webview disposed"));
    for (const waiter of [...this.readyWaiters]) {
      clearTimeout(waiter.timeout);
      waiter.reject(new Error("Tomcat webview disposed"));
      this.readyWaiters.delete(waiter);
    }
  }

  resolveWebviewView(view: vscode.WebviewView): void | Thenable<void> {
    this.view = view;
    this.isReady = false;
    this.stateStore.setReady(false);
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.deps.extensionUri, "gui", "dist"),
        vscode.Uri.joinPath(this.deps.extensionUri, "media"),
      ],
    };
    view.webview.html = this.renderHtml(view.webview);
    this.messageSubscription?.dispose();
    this.messageSubscription = view.webview.onDidReceiveMessage((message: unknown) => {
      void this.handleWebviewMessage(message);
    });
    view.onDidChangeVisibility(() => {
      if (view.visible) {
        void this.postState();
      }
    });
  }

  async waitUntilReady(timeoutMs = 15_000): Promise<void> {
    if (this.isReady) {
      return;
    }
    return new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.readyWaiters.delete(waiter);
        reject(new Error("Timed out waiting for the Tomcat webview to become ready"));
      }, timeoutMs).unref();
      const waiter = { reject, resolve, timeout };
      this.readyWaiters.add(waiter);
    });
  }

  async captureDomSnapshot(): Promise<DomSnapshot> {
    await this.waitUntilReady();
    const messageId = createHostFrameMessageId("webview-dom");
    const pending = this.domSnapshots.create(messageId, 10_000);
    await this.postMessage({
      channel: "event",
      content: { type: "__test.capture_dom" },
      messageId,
    });
    return pending;
  }

  reveal(preserveFocus = false): void {
    this.view?.show(preserveFocus);
  }

  async dispatchTestIntent(intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>): Promise<void> {
    await this.handleIntent(intent);
  }

  setUiMode(mode: TomcatUiMode): void {
    this.uiMode = mode;
    this.stateStore.setUiMode(mode);
    if (mode === "participant") {
      this.deps.ownership.releaseAll("webview");
    }
    if (mode !== "participant" && this.isReady && !this.currentState().activeSessionId) {
      void this.bootstrap();
      return;
    }
    void this.postState();
  }

  async askUser(
    request: AskQuestionWireRequest,
    sessionId?: string | null,
  ): Promise<AskQuestionWireResponse> {
    const responsePromise = new Promise<AskQuestionWireResponse>((resolve) => {
      this.pendingQuestions.set(request.requestId, { request, resolve, sessionId });
    }).finally(() => {
      this.pendingQuestions.delete(request.requestId);
      this.stateStore.resolveApproval(request.requestId);
      void this.postState();
    });

    this.stateStore.applyEvent({
      payload: request,
      requestId: request.requestId,
      sessionId,
      subtype: "ask_question",
      type: "control_request",
    });
    await this.postEvent({
      payload: request,
      requestId: request.requestId,
      sessionId,
      subtype: "ask_question",
      type: "control_request",
    });
    await this.postState();
    return responsePromise;
  }

  currentState() {
    return this.stateStore.snapshot();
  }

  private async bootstrap(): Promise<void> {
    await this.ensureInitialized();
    await this.refreshModels();
    const sessions = await this.sessionPool.refresh();
    this.stateStore.syncSessionList(sessions, this.deps.ownership.snapshot(), "webview");
    const preferredSessionId =
      this.sessionPool.pickDefaultSession(sessions) ??
      this.initialized?.sessionId ??
      null;
    if (!preferredSessionId) {
      const sessionId = await this.sessionPool.createSession(this.deps.getDefaultCwd());
      await this.selectSession(sessionId);
      return;
    }
    const claimed = this.claimWebviewOwner(preferredSessionId);
    if (claimed) {
      await this.selectSession(preferredSessionId);
      return;
    }
    this.stateStore.setActiveSession(preferredSessionId);
    await this.refreshSessionState(preferredSessionId);
    await this.postState();
  }

  private claimWebviewOwner(sessionId: string): boolean {
    const result = this.deps.ownership.claim(sessionId, "webview");
    if (!result.ok) {
      this.stateStore.setOwnership(sessionId, result.record.owner, "webview");
      this.stateStore.setConflict(
        sessionId,
        "This session is currently owned by the Tomcat participant.",
      );
      return false;
    }
    this.stateStore.setConflict(sessionId, null);
    this.stateStore.setOwnership(sessionId, "webview", "webview");
    return true;
  }

  private async ensureInitialized(): Promise<InitializeResult> {
    if (this.initialized) {
      return this.initialized;
    }
    this.initialized = await this.deps.initialize();
    return this.initialized;
  }

  private async handleIntent(intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>): Promise<void> {
    if (intent.type !== "ready" && this.uiMode === "participant") {
      await this.postState();
      return;
    }

    switch (intent.type) {
      case "ready":
        this.isReady = true;
        this.stateStore.setReady(true);
        for (const waiter of [...this.readyWaiters]) {
          clearTimeout(waiter.timeout);
          waiter.resolve();
          this.readyWaiters.delete(waiter);
        }
        if (this.uiMode === "participant") {
          await this.postState();
          return;
        }
        await this.bootstrap();
        return;
      case "listSessions":
        await this.refreshSessions();
        return;
      case "newSession": {
        await this.ensureInitialized();
        const sessionId = await this.sessionPool.createSession(
          intent.data?.cwd ?? this.deps.getDefaultCwd(),
        );
        this.claimWebviewOwner(sessionId);
        await this.selectSession(sessionId);
        return;
      }
      case "switchSession":
        await this.switchSessionView(intent.data.sessionId);
        return;
      case "closeSession": {
        const closed = await this.sessionPool.release(intent.data.sessionId);
        if (closed) {
          this.deps.ownership.release(intent.data.sessionId, "webview");
          await this.refreshSessions();
          const fallback = this.sessionPool.pickDefaultSession(this.currentStateToSessionList());
          if (fallback) {
            await this.refreshSessionState(fallback);
            this.stateStore.setActiveSession(fallback);
          } else {
            this.stateStore.setActiveSession(null);
          }
          await this.postState();
        }
        return;
      }
      case "prompt":
      case "steer": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        const response = await this.deps.messenger.request({
          params: {
            attachments: [],
          },
          sessionId,
          text: intent.data.text,
          type: intent.type === "prompt" ? "prompt" : "steer",
        });
        if (!response.success) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            response.error ?? `Tomcat ${intent.type} failed`,
          );
        }
        this.stateStore.setActiveSession(sessionId);
        await this.refreshSessionState(sessionId);
        await this.postState();
        return;
      }
      case "interrupt": {
        const sessionId = intent.data?.sessionId ?? this.currentState().activeSessionId;
        if (!sessionId) {
          return;
        }
        await this.deps.messenger.request({
          sessionId,
          type: "interrupt",
        });
        return;
      }
      case "setModel": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        const response = await this.deps.messenger.sendSetModel(sessionId, intent.data.modelId);
        if (!response.success) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            response.error ?? "Unable to switch model",
          );
        }
        await this.refreshModels();
        await this.refreshSessionState(sessionId);
        await this.postState();
        return;
      }
      case "setPlanMode": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        const response = await this.deps.messenger.sendSetPlanMode({
          action: intent.data.action,
          planId: intent.data.planId,
          sessionId,
        });
        if (!response.success) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            response.error ?? "Unable to change plan mode",
          );
        }
        await this.refreshSessionState(sessionId);
        await this.postState();
        return;
      }
      case "openDiff":
        await this.deps.ide.openPreparedDiff(intent.data.toolCallId);
        return;
      case "applyEdit":
        await this.deps.ide.applyPreparedEdit(intent.data.toolCallId);
        return;
      case "answerQuestion": {
        const pending = this.pendingQuestions.get(intent.data.requestId);
        if (!pending) {
          return;
        }
        pending.resolve(
          normalizeAskQuestionResponse(intent.data.requestId, intent.data.result),
        );
        return;
      }
    }
  }

  private async handleServeEvent(event: ServeEvent): Promise<void> {
    if (event.type === "tool_execution_start" && isMutationTool(event.toolName)) {
      await this.deps.ide.rememberToolStart(event.toolCallId, event.args);
    }
    if (
      event.type === "tool_execution_end" &&
      isMutationTool(event.toolName) &&
      event.display?.kind === "file"
    ) {
      await this.deps.ide.rememberToolResult(event.toolCallId, event.display.file);
    }

    this.stateStore.applyEvent(event);
    if (event.sessionId) {
      this.stateStore.setOwnership(
        event.sessionId,
        this.deps.ownership.ownerOf(event.sessionId)?.owner ?? null,
        "webview",
      );
    }
    await this.postEvent(event);
    await this.postState();
  }

  private async handleWebviewMessage(message: unknown): Promise<void> {
    if (!isWebviewIntent(message)) {
      return;
    }
    if (message.type === "__test.dom_snapshot") {
      this.domSnapshots.resolve(message.messageId, message.data);
      return;
    }
    await this.handleIntent(message);
  }

  private currentStateToSessionList() {
    return {
      activeSessionId: this.currentState().activeSessionId,
      scope: "disk" as const,
      sessions: this.currentState().sessions.map((session) => ({
        busy: session.busy,
        isCurrent: session.isCurrent,
        sessionId: session.sessionId,
        updatedAt: session.updatedAt,
      })),
    };
  }

  private async ensureWebviewSession(sessionId: string | null): Promise<string | null> {
    const target = sessionId ?? this.currentState().activeSessionId;
    if (!target) {
      const created = await this.sessionPool.createSession(this.deps.getDefaultCwd());
      this.claimWebviewOwner(created);
      await this.selectSession(created);
      return created;
    }
    if (!this.claimWebviewOwner(target)) {
      return null;
    }
    await this.selectSession(target);
    return target;
  }

  private async postEvent(content: HostEventFrameContent): Promise<void> {
    await this.postMessage({
      channel: "event",
      content,
      messageId: createHostFrameMessageId("event"),
    });
  }

  private async postMessage(frame: HostToWebviewFrame): Promise<void> {
    if (!this.view) {
      return;
    }
    await this.view.webview.postMessage(frame);
  }

  private async postState(): Promise<void> {
    if (!this.view || !this.isReady) {
      return;
    }
    await this.postMessage({
      channel: "state",
      content: this.stateStore.snapshot(),
      messageId: createHostFrameMessageId("state"),
    });
  }

  private refreshHtml(): void {
    if (!this.view) {
      return;
    }
    this.view.webview.html = this.renderHtml(this.view.webview);
  }

  private async refreshModels(): Promise<void> {
    const initializeResult = await this.ensureInitialized();
    if (!hasServeCapability(initializeResult, SERVE_CAPABILITY_LIST_MODELS)) {
      this.stateStore.setAvailableModels([]);
      return;
    }
    const response = await this.deps.messenger.sendListModels();
    if (!response.success) {
      this.stateStore.setAvailableModels([]);
      return;
    }
    this.stateStore.setAvailableModels(parseModelIds(response.payload));
  }

  private async refreshSessions(): Promise<void> {
    await this.ensureInitialized();
    const sessions = await this.sessionPool.refresh();
    this.stateStore.syncSessionList(sessions, this.deps.ownership.snapshot(), "webview");
    await this.postState();
  }

  private async refreshSessionState(sessionId: string): Promise<void> {
    const state = await this.deps.sessionRouter.getState(sessionId).catch(() => null);
    if (!state) {
      return;
    }
    this.stateStore.applySessionState(
      state,
      this.deps.ownership.ownerOf(sessionId)?.owner ?? null,
      "webview",
    );
  }

  private renderHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.deps.extensionUri.fsPath, "gui", "dist");
    const jsPath = path.join(distRoot, "index.js");
    const cssPath = path.join(distRoot, "index.css");
    if (!fs.existsSync(jsPath)) {
      return this.renderFallbackHtml(
        "Tomcat webview assets are missing. Run `npm run build` in `tomcat-vscode-ext` to generate `gui/dist`.",
      );
    }

    const scriptUri = webview.asWebviewUri(vscode.Uri.file(jsPath));
    const styleUri = fs.existsSync(cssPath)
      ? webview.asWebviewUri(vscode.Uri.file(cssPath)).toString()
      : null;
    const nonce = getNonce();
    const styleTag = styleUri
      ? `<link rel="stylesheet" href="${styleUri}" />`
      : "";

    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta
      http-equiv="Content-Security-Policy"
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';"
    />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    ${styleTag}
    <title>Tomcat</title>
  </head>
  <body>
    <div id="root"></div>
    <script nonce="${nonce}" type="module" src="${scriptUri}"></script>
  </body>
</html>`;
  }

  private renderFallbackHtml(message: string): string {
    return `<!DOCTYPE html>
<html lang="en">
  <body>
    <pre>${message}</pre>
  </body>
</html>`;
  }

  private async selectSession(sessionId: string): Promise<void> {
    await this.ensureInitialized();
    this.stateStore.setActiveSession(sessionId);
    if (this.deps.ownership.ownerOf(sessionId)?.owner === "webview") {
      await this.sessionPool.switchTo(sessionId);
    }
    await this.refreshSessionState(sessionId);
    await this.refreshSessions();
    await this.postState();
  }

  private async switchSessionView(sessionId: string): Promise<void> {
    await this.ensureInitialized();
    const claimed = this.claimWebviewOwner(sessionId);
    if (claimed) {
      await this.sessionPool.switchTo(sessionId);
    }
    await this.refreshSessionState(sessionId);
    await this.refreshSessions();
    // Keep the user-selected session visible even when it cannot be claimed.
    this.stateStore.setActiveSession(sessionId);
    await this.postState();
  }
}
