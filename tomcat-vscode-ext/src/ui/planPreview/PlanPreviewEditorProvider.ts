import fs from "node:fs";
import path from "node:path";

import * as vscode from "vscode";

import { TOMCAT_CONFIG_SECTION, TOMCAT_PLAN_TOOLBAR_STYLE_SETTING } from "../../constants";
import {
  hasServeCapability,
  type InitializeResult,
  SERVE_CAPABILITY_LIST_MODELS,
  SERVE_CAPABILITY_SET_PLAN_MODE,
} from "../../serveClient/initialize";
import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import {
  isPlanPreviewDomSnapshotReply,
  isPlanPreviewIntent,
  type PlanEditorMode,
  type PlanFileState,
  type PlanPreviewDomAction,
  type PlanPreviewDomSnapshot,
  type PlanPreviewHostFrame,
  type PlanPreviewIntent,
  type PlanPreviewStateSnapshot,
  type PlanToolbarStyle,
} from "../../shared/planPreviewProtocol";
import { resolveGuiStylesheet } from "../guiAssets";
import { parseModelCatalog } from "../webview/provider";
import { PendingMessageTracker } from "../webview/protocol";
import { parsePlanDocument } from "./planDocument";

export const PLAN_PREVIEW_VIEW_TYPE = "tomcat.planPreview";
export const PLAN_BUILD_MODEL_SETTING = "plan.buildModel";

/** Snapshot of the plan editor VS Code currently has focused (drives context keys). */
export interface PlanActivePanelInfo {
  canBuild: boolean;
  mode: PlanEditorMode;
  path: string;
}

function normalizeToolbarStyle(value: unknown): PlanToolbarStyle {
  return value === "native" ? "native" : "hybrid";
}

function getNonce(): string {
  return Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
}

/** `state ∈ {planning, pending}` and serve exposes `set_plan_mode`. */
export function deriveCanBuild(state: PlanFileState | null, hasSetPlanModeCapability: boolean): boolean {
  if (!hasSetPlanModeCapability) {
    return false;
  }
  return state === "planning" || state === "pending";
}

export type PlanLinkTarget =
  | { href: string; kind: "external" }
  | { kind: "ignore" }
  | { kind: "file"; path: string };

function hasUriScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(value);
}

/**
 * Decide how a link inside the rendered plan body should be handled. Pure and
 * exported for unit testing: external URLs open in the browser, anchors are
 * ignored, and everything else resolves relative to the plan file on disk.
 */
export function classifyPlanLink(href: string, planPath: string): PlanLinkTarget {
  const trimmed = href.trim();
  if (!trimmed || trimmed.startsWith("#")) {
    return { kind: "ignore" };
  }
  if (/^https?:\/\//i.test(trimmed) || trimmed.startsWith("mailto:")) {
    return { href: trimmed, kind: "external" };
  }
  if (hasUriScheme(trimmed)) {
    return { href: trimmed, kind: "external" };
  }
  const withoutAnchor = trimmed.split("#")[0];
  if (!withoutAnchor) {
    return { kind: "ignore" };
  }
  const resolved = path.isAbsolute(withoutAnchor)
    ? withoutAnchor
    : path.resolve(path.dirname(planPath), withoutAnchor);
  return { kind: "file", path: resolved };
}

export interface PlanPreviewDocumentLike {
  getText(): string;
  path: string;
}

/** 1-based inclusive line range of a selection inside the plan source. */
export interface PlanSelectionLineRange {
  lineEnd: number;
  lineStart: number;
}

export interface PlanPreviewEditorProviderDeps {
  /** Insert the given plan-preview selection into the Tomcat chat as a reference. */
  addSelectionToChat(
    planPath: string,
    text: string,
    lineRange?: PlanSelectionLineRange,
  ): Promise<void> | void;
  /** Kick off a plan build for the given planId (host owns session + model). */
  buildPlan(planId: string | null): Promise<void> | void;
  ensureInitialized(): Promise<InitializeResult>;
  extensionUri: vscode.Uri;
  /** Current global build model (`tomcat.plan.buildModel`), "" when unset. */
  getBuildModel(): string;
  messenger: TomcatMessenger;
  openExternal(href: string): Promise<void> | void;
  openInTextEditor(planPath: string): Promise<void> | void;
  openWorkspaceFile(filePath: string): Promise<void> | void;
  /** Persist the global build model to `settings.json` (Global scope). */
  setBuildModel(modelId: string): Promise<void> | void;
}

interface PlanPanelEntry {
  getText(): string;
  panel: vscode.WebviewPanel;
}

export class PlanPreviewEditorProvider
  implements vscode.CustomTextEditorProvider, vscode.Disposable
{
  static readonly viewType = PLAN_PREVIEW_VIEW_TYPE;

  /** Live panels keyed by document fsPath, so commands + E2E hooks can target one. */
  private readonly panels = new Map<string, PlanPanelEntry>();
  /** Per-panel editor mode (host-owned so the native "..." menu can show the ✓). */
  private readonly panelModes = new Map<string, PlanEditorMode>();
  /** Latest derived `canBuild` per panel, so context keys stay in sync. */
  private readonly panelCanBuild = new Map<string, boolean>();
  /** fsPath of the plan editor VS Code currently has focused, or null. */
  private activePanelPath: string | null = null;
  private readonly domSnapshots = new PendingMessageTracker<PlanPreviewDomSnapshot>();
  private readonly activeEmitter = new vscode.EventEmitter<PlanActivePanelInfo | null>();

  /** Fires whenever the focused plan editor (or its mode/canBuild) changes. */
  readonly onDidChangeActivePlan = this.activeEmitter.event;

  constructor(private readonly deps: PlanPreviewEditorProviderDeps) {}

  dispose(): void {
    this.activeEmitter.dispose();
  }

  resolveCustomTextEditor(
    document: vscode.TextDocument,
    webviewPanel: vscode.WebviewPanel,
  ): void {
    webviewPanel.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.deps.extensionUri, "gui", "dist")],
    };
    webviewPanel.webview.html = this.renderHtml(webviewPanel.webview);

    const fsPath = document.uri.fsPath;
    this.panels.set(fsPath, { getText: () => document.getText(), panel: webviewPanel });
    if (!this.panelModes.has(fsPath)) {
      this.panelModes.set(fsPath, "preview");
    }
    if (webviewPanel.active) {
      this.activePanelPath = fsPath;
    }

    const post = () => this.postFor(fsPath);

    const doc: PlanPreviewDocumentLike = {
      getText: () => document.getText(),
      path: document.uri.fsPath,
    };

    const messageSub = webviewPanel.webview.onDidReceiveMessage((message: unknown) => {
      if (isPlanPreviewDomSnapshotReply(message)) {
        this.domSnapshots.resolve(message.messageId, message.data);
        return;
      }
      if (!isPlanPreviewIntent(message)) {
        return;
      }
      void this.handleIntent(message, doc, post);
    });
    const changeSub = vscode.workspace.onDidChangeTextDocument((event) => {
      if (event.document.uri.toString() === document.uri.toString()) {
        void post();
      }
    });
    const configSub = vscode.workspace.onDidChangeConfiguration((event) => {
      if (
        event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.${PLAN_BUILD_MODEL_SETTING}`) ||
        event.affectsConfiguration(`${TOMCAT_CONFIG_SECTION}.${TOMCAT_PLAN_TOOLBAR_STYLE_SETTING}`)
      ) {
        void post();
      }
    });
    const viewStateSub = webviewPanel.onDidChangeViewState(() => {
      if (webviewPanel.active) {
        this.activePanelPath = fsPath;
        this.emitActive();
      } else if (this.activePanelPath === fsPath) {
        this.activePanelPath = null;
        this.emitActive();
      }
    });
    webviewPanel.onDidDispose(() => {
      messageSub.dispose();
      changeSub.dispose();
      configSub.dispose();
      viewStateSub.dispose();
      if (this.panels.get(fsPath)?.panel === webviewPanel) {
        this.panels.delete(fsPath);
        this.panelModes.delete(fsPath);
        this.panelCanBuild.delete(fsPath);
      }
      if (this.activePanelPath === fsPath) {
        this.activePanelPath = null;
        this.emitActive();
      }
    });

    void post();
  }

  /** Build a plan for whichever plan editor is focused (native title-bar Build). */
  async runBuildForActive(): Promise<void> {
    const entry = this.activePanelPath ? this.panels.get(this.activePanelPath) : undefined;
    if (!entry) {
      return;
    }
    const { planId } = parsePlanDocument(entry.getText());
    await this.deps.buildPlan(planId);
  }

  /**
   * Ask the focused plan webview to read its live DOM selection and reply with
   * an `addSelectionToChat` intent. Used by the right-click command, since the
   * host cannot see a webview's text selection directly.
   */
  async requestCaptureSelection(): Promise<void> {
    const path = this.activePanelPath;
    const entry = path ? this.panels.get(path) : undefined;
    if (!entry) {
      return;
    }
    const frame: PlanPreviewHostFrame = {
      channel: "event",
      content: { type: "captureSelectionForChat" },
      messageId: `plan-capture-selection-${Date.now()}`,
    };
    await entry.panel.webview.postMessage(frame);
  }

  /** Switch the focused plan editor's mode; re-posts + refreshes context keys. */
  async setModeForActive(mode: PlanEditorMode): Promise<void> {
    const path = this.activePanelPath;
    if (!path || !this.panels.has(path)) {
      return;
    }
    this.panelModes.set(path, mode);
    await this.postFor(path);
  }

  /** Info about the focused plan editor (for seeding context keys). */
  getActivePlanInfo(): PlanActivePanelInfo | null {
    const path = this.activePanelPath;
    if (!path || !this.panels.has(path)) {
      return null;
    }
    return {
      canBuild: this.panelCanBuild.get(path) ?? false,
      mode: this.panelModes.get(path) ?? "preview",
      path,
    };
  }

  /** Current global build model (`tomcat.plan.buildModel`), "" when unset. */
  getBuildModel(): string {
    return this.deps.getBuildModel();
  }

  /** Persist the global build model; the config listener re-posts open panels. */
  async setBuildModel(modelId: string): Promise<void> {
    await this.deps.setBuildModel(modelId);
  }

  /** Ready model ids exposed by the serve, for the native QuickPick. */
  getAvailableModels(): Promise<string[]> {
    return this.fetchAvailableModels();
  }

  private emitActive(): void {
    this.activeEmitter.fire(this.getActivePlanInfo());
  }

  private readToolbarStyle(): PlanToolbarStyle {
    return normalizeToolbarStyle(
      vscode.workspace
        .getConfiguration(TOMCAT_CONFIG_SECTION)
        .get<string>(TOMCAT_PLAN_TOOLBAR_STYLE_SETTING, "hybrid"),
    );
  }

  private async postFor(path: string): Promise<void> {
    const entry = this.panels.get(path);
    if (!entry) {
      return;
    }
    const snapshot = await this.buildState(entry.getText(), path, {
      mode: this.panelModes.get(path) ?? "preview",
      toolbarStyle: this.readToolbarStyle(),
    });
    this.panelCanBuild.set(path, snapshot.canBuild);
    const frame: PlanPreviewHostFrame = {
      channel: "state",
      content: snapshot,
      messageId: `plan-state-${Date.now()}`,
    };
    await entry.panel.webview.postMessage(frame);
    if (path === this.activePanelPath) {
      this.emitActive();
    }
  }

  /** Test-only: capture the rendered DOM of the panel showing `planPath`. */
  async captureDomSnapshot(planPath: string): Promise<PlanPreviewDomSnapshot> {
    const panel = this.requirePanel(planPath);
    const messageId = `plan-dom-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const pending = this.domSnapshots.create(messageId, 10_000);
    const frame: PlanPreviewHostFrame = {
      channel: "event",
      content: { type: "__test.capture_dom" },
      messageId,
    };
    await panel.webview.postMessage(frame);
    return pending;
  }

  /** Test-only: drive a DOM interaction in the panel showing `planPath`. */
  async dispatchDomAction(planPath: string, action: PlanPreviewDomAction): Promise<void> {
    const panel = this.requirePanel(planPath);
    const frame: PlanPreviewHostFrame = {
      channel: "event",
      content: { action, type: "__test.dom_action" },
      messageId: `plan-dom-action-${Date.now()}`,
    };
    await panel.webview.postMessage(frame);
  }

  private requirePanel(planPath: string): vscode.WebviewPanel {
    const entry = this.panels.get(planPath);
    if (!entry) {
      throw new Error(`No plan preview panel is open for ${planPath}`);
    }
    return entry.panel;
  }

  /** Pure-ish: text + path (+ host UI state) → the snapshot the webview renders. */
  async buildState(
    text: string,
    planPath: string,
    ui: { mode: PlanEditorMode; toolbarStyle: PlanToolbarStyle } = {
      mode: "preview",
      toolbarStyle: "hybrid",
    },
  ): Promise<PlanPreviewStateSnapshot> {
    const parsed = parsePlanDocument(text);
    const availableModels = await this.fetchAvailableModels();
    const rawBuildModel = this.deps.getBuildModel();
    const buildModel =
      rawBuildModel && availableModels.length > 0 && !availableModels.includes(rawBuildModel)
        ? ""
        : rawBuildModel;
    const canBuild = deriveCanBuild(parsed.state, await this.hasSetPlanModeCapability());
    return {
      availableModels,
      bodyMarkdown: parsed.bodyMarkdown,
      buildModel,
      canBuild,
      mode: ui.mode,
      overview: parsed.overview,
      path: planPath,
      planId: parsed.planId,
      raw: parsed.raw,
      state: parsed.state,
      title: parsed.title,
      todos: parsed.todos,
      toolbarStyle: ui.toolbarStyle,
    };
  }

  /** Pure-ish: dispatch a webview intent using injected deps. Unit tested. */
  async handleIntent(
    intent: PlanPreviewIntent,
    doc: PlanPreviewDocumentLike,
    postState: () => Promise<void>,
  ): Promise<void> {
    switch (intent.type) {
      case "plan.ready":
        await postState();
        return;
      case "openInTextEditor":
        await this.deps.openInTextEditor(doc.path);
        return;
      case "openLink": {
        const target = classifyPlanLink(intent.data.href, doc.path);
        if (target.kind === "external") {
          await this.deps.openExternal(target.href);
        } else if (target.kind === "file") {
          try {
            await this.deps.openWorkspaceFile(target.path);
          } catch {
            await this.deps.openExternal(intent.data.href);
          }
        }
        return;
      }
      case "setBuildModel":
        await this.deps.setBuildModel(intent.data.modelId);
        await postState();
        return;
      case "build": {
        const { planId } = parsePlanDocument(doc.getText());
        await this.deps.buildPlan(planId);
        return;
      }
      case "addSelectionToChat": {
        const { lineEnd, lineStart, text } = intent.data;
        const lineRange =
          typeof lineStart === "number" && typeof lineEnd === "number"
            ? { lineEnd, lineStart }
            : undefined;
        await this.deps.addSelectionToChat(doc.path, text, lineRange);
        return;
      }
    }
  }

  private async hasSetPlanModeCapability(): Promise<boolean> {
    try {
      const init = await this.deps.ensureInitialized();
      return hasServeCapability(init, SERVE_CAPABILITY_SET_PLAN_MODE);
    } catch {
      return false;
    }
  }

  private async fetchAvailableModels(): Promise<string[]> {
    try {
      const init = await this.deps.ensureInitialized();
      if (!hasServeCapability(init, SERVE_CAPABILITY_LIST_MODELS)) {
        return [];
      }
      const response = await this.deps.messenger.sendListModels().catch(() => null);
      if (!response || !response.success) {
        return [];
      }
      return parseModelCatalog(response.payload).ids;
    } catch {
      return [];
    }
  }

  private renderHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.deps.extensionUri.fsPath, "gui", "dist");
    const jsPath = path.join(distRoot, "plan.js");
    const cssPath = resolveGuiStylesheet(distRoot);
    if (!fs.existsSync(jsPath)) {
      return `<!DOCTYPE html>
<html lang="en">
  <body>
    <pre>Tomcat plan preview assets are missing. Run \`npm run build\` in \`tomcat-vscode-ext\` first.</pre>
  </body>
</html>`;
    }
    const scriptUri = webview.asWebviewUri(vscode.Uri.file(jsPath));
    const styleUri = cssPath ? webview.asWebviewUri(vscode.Uri.file(cssPath)).toString() : null;
    const nonce = getNonce();
    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta
      http-equiv="Content-Security-Policy"
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}' 'strict-dynamic';"
    />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    ${styleUri ? `<link rel="stylesheet" href="${styleUri}" />` : ""}
    <title>Tomcat Plan Preview</title>
  </head>
  <body>
    <div id="root"></div>
    <script nonce="${nonce}" type="module" src="${scriptUri}"></script>
  </body>
</html>`;
  }
}
