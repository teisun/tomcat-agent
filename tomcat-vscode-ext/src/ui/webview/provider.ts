import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import * as vscode from "vscode";

import { TOMCAT_CONFIG_SECTION } from "../../constants";
import type { VsCodeIde } from "../../ide/VsCodeIde";
import {
  hasAnyModelAdminCapability,
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
import type { ServeContentSegment, ServeEvent } from "../../serveClient/wire";
import {
  createHostFrameMessageId,
  isWebviewIntent,
  PendingMessageTracker,
  type FileDiffLine,
  type FrontendOwnerKind,
  type HostEventFrameContent,
  type HostToWebviewFrame,
  type TomcatUiMode,
  type WebviewApprovalCard,
  type WebviewDomAction,
  type WebviewMessageBlock,
  type WebviewMessageSegment,
  type WebviewPendingAttachment,
  type WebviewIntent,
  type WebviewPlanFileCard,
  type WebviewReference,
  type WebviewStateSnapshot,
  type WebviewToolCard,
} from "./protocol";
import { resolveWebviewEntryAssets } from "../guiAssets";
import { parsePlanDocument } from "../planPreview/planDocument";
import { ContextSearchService } from "./contextSearch";
import { buildFileReference } from "./contextReferences";
import { SessionOwnershipTracker } from "./ownership";
import { TomcatSessionPool } from "./sessionPool";
import { WebviewStateStore } from "./state";

const HISTORY_PAGE_ENTRIES = 80;

type PendingQuestion = {
  request: AskQuestionWireRequest;
  resolve(response: AskQuestionWireResponse): void;
  sessionId?: string | null;
};

type DomSnapshot = Extract<
  WebviewIntent,
  { type: "__test.dom_snapshot" }
>["data"];

type UserSubmitKind = "prompt" | "steer";

function isMutationTool(toolName: string): boolean {
  return toolName === "write" || toolName === "edit" || toolName === "hashline_edit";
}

function reconstructDiffPair(diff: FileDiffLine[]): { after: string; before: string } {
  const before: string[] = [];
  const after: string[] = [];
  for (const line of diff) {
    if (line.tag !== "add") {
      before.push(line.text);
    }
    if (line.tag !== "del") {
      after.push(line.text);
    }
  }
  return {
    after: after.join("\n"),
    before: before.join("\n"),
  };
}

export interface TomcatWebviewProviderDeps {
  extensionUri: vscode.Uri;
  getDefaultCwd(): string | undefined;
  getUiMode(): TomcatUiMode;
  ide: VsCodeIde;
  initialize(): Promise<InitializeResult>;
  messenger: TomcatMessenger;
  openModelSettings?(route?: "models"): void;
  ownership: SessionOwnershipTracker;
  sessionRouter: SessionRouter;
  showOpenDialog?(
    options: vscode.OpenDialogOptions,
  ): Thenable<readonly vscode.Uri[] | undefined> | readonly vscode.Uri[] | undefined;
}

function getNonce(): string {
  return Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
}

function parseCapabilityNames(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.filter((entry): entry is string => typeof entry === "string");
  }
  if (typeof value !== "object" || value === null) {
    return [];
  }
  return Object.entries(value)
    .filter(([, enabled]) => enabled === true)
    .map(([name]) => name);
}

export function parseModelCatalog(payload: unknown): {
  capabilities: Record<string, string[]>;
  ids: string[];
} {
  if (typeof payload !== "object" || payload === null) {
    return { capabilities: {}, ids: [] };
  }
  const models = (payload as { models?: unknown }).models;
  if (!Array.isArray(models)) {
    return { capabilities: {}, ids: [] };
  }
  const ids: string[] = [];
  const capabilities: Record<string, string[]> = {};
  for (const entry of models) {
    if (typeof entry !== "object" || entry === null || typeof (entry as { id?: unknown }).id !== "string") {
      continue;
    }
    if ((entry as { keyPresent?: unknown }).keyPresent === false) {
      continue;
    }
    const id = (entry as { id: string }).id;
    ids.push(id);
    capabilities[id] = parseCapabilityNames((entry as { capabilities?: unknown }).capabilities);
  }
  return { capabilities, ids };
}

function guessMimeType(filePath: string): string {
  switch (path.extname(filePath).toLowerCase()) {
    case ".png":
      return "image/png";
    case ".jpg":
    case ".jpeg":
      return "image/jpeg";
    case ".gif":
      return "image/gif";
    case ".webp":
      return "image/webp";
    case ".svg":
      return "image/svg+xml";
    case ".md":
      return "text/markdown";
    case ".txt":
      return "text/plain";
    case ".json":
      return "application/json";
    case ".pdf":
      return "application/pdf";
    default:
      return "application/octet-stream";
  }
}

function inferAttachmentKind(mimeType: string): "file" | "image" {
  return mimeType.startsWith("image/") ? "image" : "file";
}

type PickedUriKind = "attachment" | "reference";

type PickedUriMetadata = {
  isDirectory: boolean;
  mimeType: string;
};

type ResolvedPickedUri =
  | {
      attachment: WebviewPendingAttachment;
      kind: "attachment";
    }
  | {
      kind: "reference";
      reference: WebviewReference;
    };

function isAttachmentMimeType(mimeType: string): boolean {
  return mimeType === "application/pdf" || mimeType.startsWith("image/");
}

function classifyPickedUriMetadata(metadata: PickedUriMetadata): PickedUriKind {
  if (metadata.isDirectory) {
    return "reference";
  }
  return isAttachmentMimeType(metadata.mimeType) ? "attachment" : "reference";
}

async function readPickedUriMetadata(uri: vscode.Uri): Promise<PickedUriMetadata> {
  const stat = await vscode.workspace.fs.stat(uri).then(
    (value) => value,
    () => null,
  );
  return {
    isDirectory: stat?.type === vscode.FileType.Directory,
    mimeType: guessMimeType(uri.fsPath || uri.path),
  };
}

export async function classifyPickedUri(uri: vscode.Uri): Promise<PickedUriKind> {
  return classifyPickedUriMetadata(await readPickedUriMetadata(uri));
}

export function buildAttachmentOpenDialogOptions(): vscode.OpenDialogOptions {
  return {
    canSelectFiles: true,
    canSelectFolders: true,
    canSelectMany: true,
    openLabel: "Add to Tomcat",
  };
}

function shouldReconcileSessionState(event: ServeEvent): boolean {
  return (
    event.type === "agent_end" ||
    event.type === "agent_interrupted" ||
    event.type === "turn_end" ||
    event.type === "plan.complete" ||
    event.type === "plan.exit" ||
    event.type === "plan.pending"
  );
}

type PlanMetadataCacheEntry = {
  mtimeMs: number;
  overview?: string;
  title?: string;
};

function expandHomePath(filePath: string): string {
  if (filePath === "~") {
    return os.homedir();
  }
  if (filePath.startsWith("~/")) {
    return path.join(os.homedir(), filePath.slice(2));
  }
  if (filePath.startsWith("$HOME/")) {
    return path.join(os.homedir(), filePath.slice("$HOME/".length));
  }
  return filePath;
}

export function parsePlanFrontmatter(
  text: string,
): Pick<WebviewPlanFileCard, "overview" | "title"> {
  const parsed = parsePlanDocument(text);
  const metadata: Pick<WebviewPlanFileCard, "overview" | "title"> = {};
  if (parsed.title) {
    metadata.title = parsed.title;
  }
  if (parsed.overview) {
    metadata.overview = parsed.overview;
  }
  return metadata;
}

export async function readPlanMetadata(
  filePath: string,
  cache: Map<string, PlanMetadataCacheEntry>,
): Promise<Pick<WebviewPlanFileCard, "overview" | "title">> {
  const resolvedPath = expandHomePath(filePath);
  try {
    const stat = await fs.promises.stat(resolvedPath);
    const cached = cache.get(filePath);
    if (cached && cached.mtimeMs === stat.mtimeMs) {
      return cached;
    }

    const text = await fs.promises.readFile(resolvedPath, "utf8");
    const metadata = parsePlanFrontmatter(text);
    cache.set(filePath, {
      ...metadata,
      mtimeMs: stat.mtimeMs,
    });
    return metadata;
  } catch {
    cache.delete(filePath);
    return {};
  }
}

function formatBridgeError(action: string, error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("Timed out waiting for response")) {
    return `Unable to ${action}: Tomcat bridge is not responding. Restart Tomcat and try again.`;
  }
  if (message.includes("tomcat serve exited")) {
    return `Unable to ${action}: Tomcat serve exited. Restart Tomcat and try again.`;
  }
  if (message.includes("TomcatMessenger has been disposed")) {
    return `Unable to ${action}: Tomcat bridge is unavailable. Restart Tomcat and try again.`;
  }
  return `Unable to ${action}: ${message}`;
}

export class TomcatWebviewViewProvider implements vscode.WebviewViewProvider, vscode.Disposable {
  private readonly contextSearch = new ContextSearchService();
  private readonly domSnapshots = new PendingMessageTracker<DomSnapshot>();
  private readonly historyFetchGen = new Map<string, number>();
  private readonly pendingQuestions = new Map<string, PendingQuestion>();
  private readonly planMetadataCache = new Map<string, PlanMetadataCacheEntry>();
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
  private lastContextSearchIntent: Extract<WebviewIntent, { type: "searchContext" }> | null = null;
  private openFileObserved = false;
  private contextSearchTokenSource?: vscode.CancellationTokenSource;
  private messageSubscription?: vscode.Disposable;
  private uiMode: TomcatUiMode;
  private visibilitySubscription?: vscode.Disposable;
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
    this.deps.ownership.releaseAll("webview");
    this.contextSearchTokenSource?.cancel();
    this.contextSearchTokenSource?.dispose();
    this.contextSearch.dispose();
    this.messageSubscription?.dispose();
    this.visibilitySubscription?.dispose();
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
    this.visibilitySubscription?.dispose();
    this.messageSubscription = view.webview.onDidReceiveMessage((message: unknown) => {
      void this.handleWebviewMessage(message);
    });
    this.visibilitySubscription = view.onDidChangeVisibility(() => {
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

  getOpenFileObserved(): boolean {
    return this.openFileObserved;
  }

  resetOpenFileObserved(): void {
    this.openFileObserved = false;
  }

  getLastContextSearchIntent(): Extract<WebviewIntent, { type: "searchContext" }> | null {
    return this.lastContextSearchIntent;
  }

  resetForTestReload(): void {
    this.isReady = false;
    this.lastContextSearchIntent = null;
    this.openFileObserved = false;
    this.planMetadataCache.clear();
    this.stateStore.resetForReload();
  }

  async dispatchTestDomAction(action: WebviewDomAction): Promise<void> {
    await this.waitUntilReady();
    await this.postMessage({
      channel: "event",
      content: { action, type: "__test.dom_action" },
      messageId: createHostFrameMessageId("webview-dom-action"),
    });
  }

  async dispatchTestHostEvent(content: HostEventFrameContent): Promise<void> {
    await this.waitUntilReady();
    await this.postEvent(content);
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

  private findToolCard(toolCallId: string): WebviewToolCard | undefined {
    for (const session of Object.values(this.currentState().sessionViews)) {
      const tool = session.timeline.find(
        (item): item is WebviewToolCard => item.type === "tool" && item.toolCallId === toolCallId,
      );
      if (tool) {
        return tool;
      }
    }
    return undefined;
  }

  async refreshModelCatalog(): Promise<void> {
    await this.refreshModels();
    if (this.isReady && this.uiMode !== "participant") {
      await this.postState();
    }
  }

  private pendingAttachmentsForSession(sessionId: string): WebviewPendingAttachment[] {
    return this.currentState().sessionViews[sessionId]?.pendingAttachments ?? [];
  }

  private async showOpenDialog(
    options: vscode.OpenDialogOptions,
  ): Promise<readonly vscode.Uri[] | undefined> {
    return this.deps.showOpenDialog?.(options) ?? vscode.window.showOpenDialog(options);
  }

  private async resolvePickedUri(uri: vscode.Uri): Promise<ResolvedPickedUri> {
    const metadata = await readPickedUriMetadata(uri);
    if (classifyPickedUriMetadata(metadata) === "attachment") {
      return {
        attachment: await this.readPendingAttachment(uri, metadata.mimeType),
        kind: "attachment",
      };
    }
    return {
      kind: "reference",
      reference: buildFileReference(uri, {
        isDirectory: metadata.isDirectory,
      }),
    };
  }

  private async ingestPickedUris(
    sessionId: string,
    uris: readonly vscode.Uri[],
  ): Promise<void> {
    const resolved = (
      await Promise.all(
        uris.map(async (uri) => {
          try {
            return await this.resolvePickedUri(uri);
          } catch {
            return null;
          }
        }),
      )
    ).filter((entry): entry is ResolvedPickedUri => entry !== null);
    if (!resolved.length) {
      return;
    }

    const existing = this.currentState().sessionViews[sessionId]?.pendingAttachments ?? [];
    const nextAttachments = [...existing];
    for (const entry of resolved) {
      if (entry.kind === "reference") {
        await this.postInsertReference(sessionId, entry.reference);
        continue;
      }
      nextAttachments.push(entry.attachment);
    }

    if (nextAttachments.length !== existing.length) {
      this.stateStore.setPendingAttachments(sessionId, nextAttachments);
      await this.postState();
    }
  }

  private lookupRetryableUserMessage(
    sessionId: string,
    messageId: string,
  ): { segments?: WebviewMessageSegment[]; submitKind: UserSubmitKind; text: string } | null {
    const session = this.currentState().sessionViews[sessionId];
    const message = session?.timeline.find(
      (item): item is WebviewMessageBlock =>
        item.type === "message" && item.kind === "user" && item.id === messageId,
    );
    if (
      !message ||
      message.deliveryState !== "failed" ||
      message.retryable !== true ||
      (message.submitKind !== "prompt" && message.submitKind !== "steer")
    ) {
      return null;
    }
    return {
      segments: message.segments,
      submitKind: message.submitKind,
      text: message.text,
    };
  }

  private async sendUserMessage(
    sessionId: string,
    submitKind: UserSubmitKind,
    text: string,
    segments?: WebviewMessageSegment[],
    options?: {
      messageId?: string;
      retrying?: boolean;
    },
  ): Promise<void> {
    const userMessageId = options?.messageId ?? randomUUID();
    const pendingAttachments =
      submitKind === "prompt" ? this.pendingAttachmentsForSession(sessionId) : [];
    this.stateStore.setActiveSession(sessionId);
    if (options?.retrying) {
      this.stateStore.markLocalUserMessagePending(sessionId, userMessageId);
    } else {
      this.stateStore.appendLocalUserMessage(sessionId, text, {
        messageId: userMessageId,
        segments,
        submitKind,
      });
    }
    await this.postState();
    try {
      const response = await this.deps.messenger.request({
        params: {
          attachments: pendingAttachments.map((entry) => entry.attachment),
          segments: segments as ServeContentSegment[] | undefined,
          userMessageId,
        },
        sessionId,
        text,
        type: submitKind,
      });
      if (!response.success) {
        this.stateStore.markLocalUserMessageFailed(
          sessionId,
          userMessageId,
          response.error ?? `Tomcat ${submitKind} failed`,
          true,
        );
      } else {
        this.stateStore.markLocalUserMessageConfirmed(sessionId, userMessageId);
        if (pendingAttachments.length) {
          this.stateStore.clearPendingAttachments(sessionId);
        }
      }
    } catch (error) {
      this.stateStore.markLocalUserMessageFailed(
        sessionId,
        userMessageId,
        formatBridgeError(
          submitKind === "prompt" ? "send the message" : "send the steering message",
          error,
        ),
        false,
      );
    }
    await this.refreshSessionState(sessionId, { trustBusy: true });
    await this.refreshSessions();
    await this.postState();
  }

  private async postContextSearchResult(
    intent: Extract<WebviewIntent, { type: "searchContext" }>,
    payload?: {
      matches: Extract<HostEventFrameContent, { type: "contextSearchResult" }>["matches"];
      truncated: boolean;
      workspaceAvailable: boolean;
    },
  ): Promise<void> {
    await this.postEvent({
      matches: payload?.matches ?? [],
      query: intent.data.query,
      requestId: intent.data.requestId,
      sessionId: intent.data.sessionId ?? null,
      truncated: payload?.truncated ?? false,
      type: "contextSearchResult",
      workspaceAvailable: payload?.workspaceAvailable,
    });
  }

  private async handleContextSearch(
    intent: Extract<WebviewIntent, { type: "searchContext" }>,
  ): Promise<void> {
    this.lastContextSearchIntent = intent;
    this.contextSearchTokenSource?.cancel();
    this.contextSearchTokenSource?.dispose();
    const tokenSource = new vscode.CancellationTokenSource();
    this.contextSearchTokenSource = tokenSource;
    try {
      const result = await this.contextSearch.search({
        kind: intent.data.kind,
        query: intent.data.query,
        token: tokenSource.token,
      });
      await this.postContextSearchResult(intent, {
        matches: result.matches,
        truncated: result.truncated,
        workspaceAvailable: result.workspaceAvailable,
      });
    } catch (error) {
      if (!tokenSource.token.isCancellationRequested) {
        console.error("Tomcat context search failed", error);
      }
      await this.postContextSearchResult(intent);
    } finally {
      if (this.contextSearchTokenSource === tokenSource) {
        this.contextSearchTokenSource = undefined;
      }
      tokenSource.dispose();
    }
  }

  private lookupApprovalSessionId(requestId: string): string | null {
    for (const session of Object.values(this.currentState().sessionViews)) {
      const approval = session.timeline.find(
        (item): item is WebviewApprovalCard =>
          item.type === "approval" && item.request.requestId === requestId,
      );
      if (approval) {
        return approval.sessionId ?? session.sessionId;
      }
    }
    return null;
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
    await this.refreshSessionState(preferredSessionId, { trustBusy: true });
    await this.refreshSessionHistory(preferredSessionId);
    await this.refreshCheckpoints(preferredSessionId);
    this.stateStore.setActiveSession(preferredSessionId);
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
      case "listCheckpoints":
        await this.refreshCheckpoints(intent.data.sessionId);
        await this.postState();
        return;
      case "loadOlderHistory":
        await this.loadOlderHistory(intent.data.sessionId);
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
            await this.refreshSessionState(fallback, { trustBusy: true });
            await this.refreshSessionHistory(fallback);
            await this.refreshCheckpoints(fallback);
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
        await this.sendUserMessage(
          sessionId,
          intent.type,
          intent.data.text,
          intent.data.segments,
          {
            messageId: intent.data.userMessageId,
          },
        );
        return;
      }
      case "retryUserMessage": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId);
        if (!sessionId) {
          await this.postState();
          return;
        }
        const retry = this.lookupRetryableUserMessage(sessionId, intent.data.messageId);
        if (!retry) {
          return;
        }
        await this.sendUserMessage(
          sessionId,
          retry.submitKind,
          retry.text,
          retry.segments,
          {
            messageId: intent.data.messageId,
            retrying: true,
          },
        );
        return;
      }
      case "resolveDrop": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSessionWithoutHistory(
          intent.data.sessionId ?? null,
        );
        if (!sessionId) {
          await this.postState();
          return;
        }
        const uris: vscode.Uri[] = [];
        for (const rawUri of intent.data.uris) {
          try {
            uris.push(vscode.Uri.parse(rawUri));
          } catch {
            // Ignore malformed drop payload entries; the editor keeps the rest.
          }
        }
        await this.ingestPickedUris(sessionId, uris);
        return;
      }
      case "searchContext":
        await this.handleContextSearch(intent);
        return;
      case "showWarningMessage":
        await vscode.window.showWarningMessage(intent.data.message);
        return;
      case "pickContext": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data?.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        const picks = await this.showOpenDialog(buildAttachmentOpenDialogOptions());
        if (!picks?.length) {
          return;
        }
        await this.ingestPickedUris(sessionId, picks);
        return;
      }
      case "removeAttachment": {
        const sessionId = intent.data.sessionId ?? this.currentState().activeSessionId;
        if (!sessionId) {
          return;
        }
        this.stateStore.removePendingAttachment(sessionId, intent.data.attachmentId);
        await this.postState();
        return;
      }
      case "interrupt": {
        const sessionId = await this.ensureWebviewSessionWithoutHistory(
          intent.data?.sessionId ?? this.currentState().activeSessionId,
        );
        if (!sessionId) {
          await this.postState();
          return;
        }
        await this.deps.messenger.request({
          sessionId,
          type: "interrupt",
        });
        return;
      }
      case "restoreCheckpoint": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSessionWithoutHistory(intent.data.sessionId);
        if (!sessionId) {
          await this.postState();
          return;
        }
        try {
          await this.deps.sessionRouter.restoreCheckpoint(
            sessionId,
            intent.data.checkpointId,
            intent.data.revertFiles,
          );
        } catch (error) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            formatBridgeError("restore checkpoint", error),
          );
          await this.postState();
          return;
        }
        await this.refreshSessionState(sessionId, { trustBusy: true });
        await this.refreshSessionHistory(sessionId);
        await this.refreshCheckpoints(sessionId);
        await this.refreshSessions();
        await this.postState();
        return;
      }
      case "setModel": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        try {
          const response = await this.deps.messenger.sendSetModel(sessionId, intent.data.modelId);
          if (!response.success) {
            this.stateStore.appendMessage(
              sessionId,
              "error",
              response.error ?? "Unable to switch model",
            );
          }
        } catch (error) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            formatBridgeError("switch models", error),
          );
        }
        await this.refreshModels();
        await this.refreshSessionState(sessionId, { trustBusy: true });
        await this.postState();
        return;
      }
      case "setThinkingLevel": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSession(intent.data.sessionId ?? null);
        if (!sessionId) {
          await this.postState();
          return;
        }
        try {
          const response = await this.deps.messenger.sendSetThinkingLevel(
            sessionId,
            intent.data.modelId,
            intent.data.level,
          );
          if (!response.success) {
            this.stateStore.appendMessage(
              sessionId,
              "error",
              response.error ?? "Unable to change reasoning effort",
            );
          }
        } catch (error) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            formatBridgeError("change reasoning effort", error),
          );
        }
        await this.refreshSessionState(sessionId, { trustBusy: true });
        await this.postState();
        return;
      }
      case "openModelSettings":
        if (!hasAnyModelAdminCapability(await this.ensureInitialized())) {
          return;
        }
        this.deps.openModelSettings?.(intent.data?.route ?? "models");
        return;
      case "setBuildModel": {
        await vscode.workspace
          .getConfiguration(TOMCAT_CONFIG_SECTION)
          .update("plan.buildModel", intent.data.modelId, vscode.ConfigurationTarget.Global);
        this.stateStore.setBuildModel(intent.data.modelId);
        await this.postState();
        return;
      }
      case "setPlanMode": {
        await this.ensureInitialized();
        const sessionId = await this.ensureWebviewSessionWithoutHistory(
          intent.data.sessionId ?? null,
        );
        if (!sessionId) {
          await this.postState();
          return;
        }
        if (intent.data.action === "build") {
          await this.runPlanBuild(sessionId, intent.data.planId);
          return;
        }
        try {
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
        } catch (error) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            formatBridgeError("change plan mode", error),
          );
        }
        await this.refreshSessionState(sessionId, { trustBusy: true });
        await this.postState();
        return;
      }
      case "openFile":
        this.openFileObserved = true;
        try {
          await this.deps.ide.showFile(intent.data.path);
        } catch (error) {
          const sessionId = this.currentState().activeSessionId;
          if (sessionId) {
            this.stateStore.appendMessage(
              sessionId,
              "error",
              formatBridgeError(`open file ${intent.data.path}`, error),
            );
            await this.postState();
          }
        }
        return;
      case "openDiff": {
        const tool = this.findToolCard(intent.data.toolCallId);
        const displayPath =
          tool?.display?.kind === "file"
            ? tool.display.file
            : typeof tool?.args?.path === "string"
              ? tool.args.path
              : null;
        if (!tool || !displayPath) {
          return;
        }
        try {
          if (this.deps.ide.getPreparedChange(intent.data.toolCallId)) {
            await this.deps.ide.openPreparedDiff(intent.data.toolCallId);
          } else if (tool.diff?.length) {
            const { after, before } = reconstructDiffPair(tool.diff);
            await this.deps.ide.openReconstructedDiff(
              intent.data.toolCallId,
              displayPath,
              before,
              after,
            );
          } else {
            await this.deps.ide.showFile(displayPath);
          }
        } catch (error) {
          const sessionId = this.currentState().activeSessionId;
          if (sessionId) {
            this.stateStore.appendMessage(
              sessionId,
              "error",
              formatBridgeError(`open diff ${displayPath}`, error),
            );
            await this.postState();
          }
        }
        return;
      }
      case "openPlanFile":
        try {
          await this.deps.ide.openWith(intent.data.path, "tomcat.planPreview");
        } catch (error) {
          try {
            await this.deps.ide.showFile(intent.data.path);
          } catch {
            const sessionId = this.currentState().activeSessionId;
            if (sessionId) {
              this.stateStore.appendMessage(
                sessionId,
                "error",
                formatBridgeError(`open plan file ${intent.data.path}`, error),
              );
              await this.postState();
            }
          }
        }
        return;
      case "answerQuestion": {
        const pending = this.pendingQuestions.get(intent.data.requestId);
        if (!pending) {
          const sessionId =
            this.lookupApprovalSessionId(intent.data.requestId)
            ?? this.currentState().activeSessionId;
          if (sessionId) {
            this.stateStore.appendMessage(
              sessionId,
              "notice",
              "This question is no longer active. Please ask again if you still need it.",
            );
            await this.postState();
          }
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
    if (
      event.type === "tool_execution_start" &&
      isMutationTool(event.toolName) &&
      typeof this.deps.ide.rememberToolStart === "function"
    ) {
      try {
        await this.deps.ide.rememberToolStart(event.toolCallId, event.args);
      } catch (error) {
        console.warn("Tomcat webview failed to capture tool start snapshot", error);
      }
    }
    if (
      event.type === "tool_execution_end" &&
      event.display?.kind === "file" &&
      isMutationTool(event.toolName) &&
      typeof this.deps.ide.rememberToolResult === "function"
    ) {
      try {
        await this.deps.ide.rememberToolResult(event.toolCallId, event.display.file);
      } catch (error) {
        console.warn("Tomcat webview failed to capture tool result snapshot", error);
      }
    }
    this.stateStore.applyEvent(event);
    if (event.sessionId) {
      this.stateStore.setOwnership(
        event.sessionId,
        this.deps.ownership.ownerOf(event.sessionId)?.owner ?? null,
        "webview",
      );
      if (shouldReconcileSessionState(event)) {
        await this.refreshSessionState(event.sessionId, { trustBusy: false });
      }
      if (event.type === "turn_end") {
        await this.refreshCheckpoints(event.sessionId);
      }
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
        title: session.title,
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

  private async ensureWebviewSessionWithoutHistory(
    sessionId: string | null,
  ): Promise<string | null> {
    const target = sessionId ?? this.currentState().activeSessionId;
    if (!target) {
      const created = await this.sessionPool.createSession(this.deps.getDefaultCwd());
      if (!this.claimWebviewOwner(created)) {
        return null;
      }
      this.stateStore.setActiveSession(created);
      await this.sessionPool.switchTo(created);
      await this.refreshSessions();
      return created;
    }

    if (!this.claimWebviewOwner(target)) {
      return null;
    }

    this.stateStore.setActiveSession(target);
    if (this.deps.ownership.ownerOf(target)?.owner === "webview") {
      await this.sessionPool.switchTo(target);
    }
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
    const snapshot = await this.enrichPlanCards(this.stateStore.snapshot());
    await this.postMessage({
      channel: "state",
      content: snapshot,
      messageId: createHostFrameMessageId("state"),
    });
  }

  async postInsertReference(sessionId: string, reference: WebviewReference): Promise<void> {
    await this.postEvent({
      reference,
      sessionId,
      type: "insertReference",
    });
  }

  private async enrichPlanCards(snapshot: WebviewStateSnapshot): Promise<WebviewStateSnapshot> {
    const sessions = Object.values(snapshot.sessionViews);
    await Promise.all(
      sessions.map(async (session) => {
        const planCards = session.timeline.filter(
          (item): item is WebviewPlanFileCard => item.type === "plan",
        );
        await Promise.all(
          planCards.map(async (item) => {
            const metadata = await readPlanMetadata(item.path, this.planMetadataCache);
            if (metadata.title) {
              item.title = metadata.title;
            } else {
              delete item.title;
            }
            if (metadata.overview) {
              item.overview = metadata.overview;
            } else {
              delete item.overview;
            }
          }),
        );
      }),
    );
    return snapshot;
  }

  private refreshHtml(): void {
    if (!this.view) {
      return;
    }
    this.view.webview.html = this.renderHtml(this.view.webview);
  }

  private readBuildModelConfig(): string {
    return (
      vscode.workspace
        .getConfiguration(TOMCAT_CONFIG_SECTION)
        .get<string>("plan.buildModel", "") ?? ""
    );
  }

  /** Re-read `tomcat.plan.buildModel` and push it to the webview (config sync). */
  async syncBuildModel(): Promise<void> {
    this.stateStore.setBuildModel(this.readBuildModelConfig());
    if (this.isReady && this.uiMode !== "participant") {
      await this.postState();
    }
  }

  /**
   * Single build path shared by the chat PlanFileCard and the plan preview
   * editor: apply the global build model (when set) before entering build mode.
   */
  private async runPlanBuild(sessionId: string, planId?: string | null): Promise<void> {
    const buildModel = this.readBuildModelConfig();
    try {
      if (buildModel) {
        const modelResponse = await this.deps.messenger.sendSetModel(sessionId, buildModel);
        if (!modelResponse.success) {
          this.stateStore.appendMessage(
            sessionId,
            "error",
            modelResponse.error ?? "Unable to switch model",
          );
        }
      }
      const response = await this.deps.messenger.sendSetPlanMode({
        action: "build",
        planId,
        sessionId,
      });
      if (!response.success) {
        this.stateStore.appendMessage(
          sessionId,
          "error",
          response.error ?? "Unable to change plan mode",
        );
      }
    } catch (error) {
      this.stateStore.appendMessage(
        sessionId,
        "error",
        formatBridgeError("change plan mode", error),
      );
    }
    if (buildModel) {
      await this.refreshModels();
    }
    await this.refreshSessionState(sessionId, { trustBusy: true });
    await this.postState();
  }

  /** Public build entry for the plan preview editor (ensures a session first). */
  async buildPlan(planId: string | null): Promise<void> {
    await this.ensureInitialized();
    const sessionId = await this.ensureWebviewSessionWithoutHistory(null);
    if (!sessionId) {
      await this.postState();
      return;
    }
    await this.runPlanBuild(sessionId, planId);
  }

  private async refreshModels(): Promise<void> {
    this.stateStore.setBuildModel(this.readBuildModelConfig());
    const initializeResult = await this.ensureInitialized();
    this.stateStore.setModelAdminSupported(
      hasAnyModelAdminCapability(initializeResult),
    );
    if (!hasServeCapability(initializeResult, SERVE_CAPABILITY_LIST_MODELS)) {
      this.stateStore.setAvailableModels([], {});
      return;
    }
    const response = await this.deps.messenger.sendListModels().catch(() => null);
    if (!response) {
      this.stateStore.setAvailableModels([], {});
      return;
    }
    if (!response.success) {
      this.stateStore.setAvailableModels([], {});
      return;
    }
    const catalog = parseModelCatalog(response.payload);
    this.stateStore.setAvailableModels(catalog.ids, catalog.capabilities);
  }

  private async refreshSessions(): Promise<void> {
    await this.ensureInitialized();
    const sessions = await this.sessionPool.refresh();
    this.stateStore.syncSessionList(sessions, this.deps.ownership.snapshot(), "webview");
    await this.postState();
  }

  private async refreshSessionState(
    sessionId: string,
    options: {
      trustBusy?: boolean;
    } = {},
  ): Promise<void> {
    const state = await this.deps.sessionRouter.getState(sessionId).catch(() => null);
    if (!state) {
      return;
    }
    this.stateStore.applySessionState(
      state,
      this.deps.ownership.ownerOf(sessionId)?.owner ?? null,
      "webview",
      { trustBusy: options.trustBusy ?? true },
    );
  }

  private bumpHistoryFetchGen(sessionId: string): number {
    const next = (this.historyFetchGen.get(sessionId) ?? 0) + 1;
    this.historyFetchGen.set(sessionId, next);
    return next;
  }

  private currentHistoryFetchGen(sessionId: string): number {
    return this.historyFetchGen.get(sessionId) ?? 0;
  }

  private async refreshSessionHistory(sessionId: string): Promise<void> {
    if (typeof this.deps.sessionRouter.getMessages !== "function") {
      return;
    }
    const fetchGen = this.bumpHistoryFetchGen(sessionId);
    const history = await this.deps.sessionRouter.getMessages(sessionId, {
      limit: HISTORY_PAGE_ENTRIES,
    }).catch(() => null);
    if (this.currentHistoryFetchGen(sessionId) !== fetchGen) {
      return;
    }
    if (!history || history.sessionId !== sessionId) {
      return;
    }
    this.stateStore.hydrateHistory(sessionId, history);
  }

  private async refreshCheckpoints(sessionId: string): Promise<void> {
    if (typeof this.deps.sessionRouter.listCheckpoints !== "function") {
      return;
    }
    const checkpoints = await this.deps.sessionRouter.listCheckpoints(sessionId).catch(() => null);
    if (!checkpoints || checkpoints.sessionId !== sessionId) {
      return;
    }
    this.stateStore.setCheckpoints(sessionId, checkpoints.checkpoints);
  }

  private async loadOlderHistory(sessionId: string): Promise<void> {
    if (typeof this.deps.sessionRouter.getMessages !== "function") {
      return;
    }
    const session = this.currentState().sessionViews[sessionId];
    if (!session?.hasMoreHistory || session.historyLoading !== false) {
      return;
    }
    const cursor = this.stateStore.getOldestHistoryCursor(sessionId);
    if (!cursor) {
      return;
    }
    const fetchGen = this.currentHistoryFetchGen(sessionId);
    this.stateStore.setHistoryLoading(sessionId, true);
    await this.postState();
    const history = await this.deps.sessionRouter.getMessages(sessionId, {
      cursor,
      limit: HISTORY_PAGE_ENTRIES,
    }).catch(() => null);
    if (this.currentHistoryFetchGen(sessionId) !== fetchGen) {
      this.stateStore.setHistoryLoading(sessionId, false);
      await this.postState();
      return;
    }
    if (!history || history.sessionId !== sessionId) {
      this.stateStore.setHistoryLoading(sessionId, false);
      await this.postState();
      return;
    }
    this.stateStore.prependHistory(sessionId, history);
    await this.postState();
  }

  private async readPendingAttachment(
    uri: vscode.Uri,
    mimeType = guessMimeType(uri.fsPath || uri.path),
  ): Promise<WebviewPendingAttachment> {
    const bytes = await vscode.workspace.fs.readFile(uri);
    return {
      attachment: {
        dataBase64: Buffer.from(bytes).toString("base64"),
        filename: path.basename(uri.fsPath),
        kind: inferAttachmentKind(mimeType),
        mimeType,
      },
      id: `${path.basename(uri.fsPath)}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      kind: inferAttachmentKind(mimeType),
      label: path.basename(uri.fsPath),
      mimeType,
      path: uri.fsPath,
    };
  }

  private renderHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.deps.extensionUri.fsPath, "gui", "dist");
    const assets = resolveWebviewEntryAssets(distRoot, "index.html", "index.js");
    if (assets.scripts.length === 0) {
      return this.renderFallbackHtml(
        "Tomcat webview assets are missing. Run `npm run build` in `tomcat-vscode-ext` to generate `gui/dist`.",
      );
    }

    const nonce = getNonce();
    const styleTags = assets.stylesheets
      .map(
        (file) =>
          `<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}" />`,
      )
      .join("\n    ");
    const scriptTags = assets.scripts
      .map(
        (file) =>
          `<script nonce="${nonce}" type="module" src="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}"></script>`,
      )
      .join("\n    ");

    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta
      http-equiv="Content-Security-Policy"
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';"
    />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    ${styleTags}
    <title>Tomcat</title>
  </head>
  <body>
    <div id="root"></div>
    ${scriptTags}
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
    await this.refreshSessionState(sessionId, { trustBusy: true });
    await this.refreshSessionHistory(sessionId);
    await this.refreshCheckpoints(sessionId);
    await this.refreshSessions();
    this.stateStore.setActiveSession(sessionId);
    await this.postState();
  }

  private async switchSessionView(sessionId: string): Promise<void> {
    await this.ensureInitialized();
    const claimed = this.claimWebviewOwner(sessionId);
    if (claimed) {
      await this.sessionPool.switchTo(sessionId);
    }
    await this.refreshSessionState(sessionId, { trustBusy: true });
    await this.refreshSessionHistory(sessionId);
    await this.refreshCheckpoints(sessionId);
    await this.refreshSessions();
    // Keep the user-selected session visible even when it cannot be claimed.
    this.stateStore.setActiveSession(sessionId);
    await this.postState();
  }
}
