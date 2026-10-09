/**
 * ImagePreviewPanel — Extension Host controller for the image preview WebviewPanel.
 *
 * Singleton per extension instance: replaces state on repeat open, creates on first use.
 * Handles:
 * - createWebviewPanel / reveal
 * - HTML generation with strict CSP
 * - ready/state/select/save/close message relay with the Chat Webview
 * - Save via showSaveDialog + workspace.fs.writeFile
 * - URL lifecycle and dispose cleanup
 */

import * as path from "node:path";
import * as vscode from "vscode";

import { attachmentResourceRoots } from "../../shared/attachmentUris";
import type {
  ImagePreviewDomAction,
  ImagePreviewDomSnapshot,
  PreviewPicture,
  PreviewSection,
} from "../../shared/imagePreviewProtocol";
import { resolveWebviewEntryAssets } from "../guiAssets";

export class ImagePreviewPanel {
  private static instance: ImagePreviewPanel | undefined;

  private panel: vscode.WebviewPanel | undefined;
  private activeId: string | null = null;
  private readonly extensionUri: vscode.Uri;
  private sections: PreviewSection[] = [];
  private pictureMap = new Map<string, PreviewPicture>();
  private ids: string[] = [];
  private testRequestSequence = 0;
  private readonly testSnapshotWaiters = new Map<
    string,
    {
      reject(error: Error): void;
      resolve(snapshot: ImagePreviewDomSnapshot): void;
      timeout: ReturnType<typeof setTimeout>;
    }
  >();

  /**
   * Filesystem root of the backend attachment store.
   *
   * The panel needs it twice: to grant itself read access to the byte directories, and
   * to read bytes back when the user saves a picture. It never holds bytes otherwise.
   */
  private attachmentRoot: string | null = null;

  static getInstance(
    extensionUri: vscode.Uri,
    attachmentRoot?: string | null,
  ): ImagePreviewPanel {
    if (!ImagePreviewPanel.instance) {
      ImagePreviewPanel.instance = new ImagePreviewPanel(extensionUri);
    }
    if (attachmentRoot) {
      ImagePreviewPanel.instance.attachmentRoot = attachmentRoot;
    }
    return ImagePreviewPanel.instance;
  }

  static getCurrent(): ImagePreviewPanel | undefined {
    return ImagePreviewPanel.instance;
  }

  private constructor(extensionUri: vscode.Uri) {
    this.extensionUri = extensionUri;
  }

  /**
   * Reveal the preview panel with the given collection and active picture.
   * - `sections`: picture groups (history turns + optional pending section)
   * - `activeId`: the picture id to show first
   *
   * If the panel already exists, it's reused and state is updated.
   */
  reveal(sections: PreviewSection[], activeId: string): void {
    activeId = this.replaceSections(sections, activeId) ?? activeId;
    this.activeId = activeId;
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active);
      this.postState(activeId);
      return;
    }

    // Create new panel
    this.panel = vscode.window.createWebviewPanel(
      "tomcat.imagePreview",
      "Image Preview",
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        // Same reasoning as the chat webview: the attachment directories sit outside
        // extensionUri, so access has to be granted explicitly or every image 404s.
        localResourceRoots: [
          vscode.Uri.file(path.join(this.extensionUri.fsPath, "gui", "dist")),
          ...attachmentResourceRoots(this.attachmentRoot),
        ],
      },
    );

    this.panel.webview.html = this.buildHtml(this.panel.webview);
    this.panel.onDidDispose(() => {
      this.panel = undefined;
      this.rejectTestSnapshotWaiters(
        new Error("Image preview panel was closed before the DOM snapshot completed"),
      );
      ImagePreviewPanel.instance = undefined;
    });

    // Listen for messages from preview webview
    this.panel.webview.onDidReceiveMessage((msg) => {
      switch (msg.type) {
        case "preview.ready":
          if (this.activeId) {
            this.postState(this.activeId);
          }
          break;        case "preview.select":
          this.postState(msg.data.attachmentId);
          break;
        case "preview.save":
          this.handleSave(msg.data.attachmentId);
          break;
        case "preview.close":
          this.panel?.dispose();
          break;
        case "preview.__test.dom_snapshot": {
          const requestId =
            typeof msg.requestId === "string" ? msg.requestId : null;
          const waiter = requestId
            ? this.testSnapshotWaiters.get(requestId)
            : undefined;
          if (!requestId || !waiter) {
            break;
          }
          clearTimeout(waiter.timeout);
          this.testSnapshotWaiters.delete(requestId);
          waiter.resolve(msg.snapshot as ImagePreviewDomSnapshot);
          break;
        }
      }
    });
  }

  /** Replace the collection without stealing editor focus. */
  updateSections(sections: PreviewSection[]): void {
    if (!this.panel) return;
    const oldIndex = this.activeId ? this.ids.indexOf(this.activeId) : 0;
    const preferredId = this.activeId ?? this.ids[0] ?? "";
    const nextActiveId = this.replaceSections(sections, preferredId, oldIndex);
    if (!nextActiveId) {
      this.close();
      return;
    }
    this.activeId = nextActiveId;
    this.postState(nextActiveId);
  }

  /** Close the panel (e.g. when session is deleted). */
  close(): void {
    if (this.panel) {
      this.panel.dispose();
      this.panel = undefined;
      ImagePreviewPanel.instance = undefined;
    }
  }

  private replaceSections(
    sections: PreviewSection[],
    preferredId: string,
    fallbackIndex = 0,
  ): string | null {
    this.sections = sections;
    this.pictureMap.clear();
    this.ids = [];
    const seen = new Set<string>();
    for (const section of sections) {
      for (const picture of section.pictures) {
        if (seen.has(picture.id)) continue;
        seen.add(picture.id);
        this.pictureMap.set(picture.id, picture);
        this.ids.push(picture.id);
      }
    }
    if (this.pictureMap.has(preferredId)) {
      return preferredId;
    }
    if (this.ids.length === 0) {
      return null;
    }
    return this.ids[Math.max(0, Math.min(fallbackIndex, this.ids.length - 1))];
  }

  /** Returns true if the panel is currently visible. */
  /** Returns true if the panel is currently visible. */
  get isVisible(): boolean {
    return this.panel !== undefined;
  }

  async __testingCaptureDom(
    timeoutMs = 5_000,
  ): Promise<ImagePreviewDomSnapshot> {
    if (!this.panel) {
      throw new Error("Image preview panel is not open");
    }
    const requestId = `image-preview-dom-${++this.testRequestSequence}`;
    const snapshot = new Promise<ImagePreviewDomSnapshot>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.testSnapshotWaiters.delete(requestId);
        reject(new Error("Timed out waiting for image preview DOM snapshot"));
      }, timeoutMs);
      this.testSnapshotWaiters.set(requestId, { reject, resolve, timeout });
    });
    await this.panel.webview.postMessage({
      requestId,
      type: "preview.__test.capture_dom",
    });
    return snapshot;
  }

  async __testingDispatchDomAction(
    action: ImagePreviewDomAction,
  ): Promise<void> {
    if (!this.panel) {
      throw new Error("Image preview panel is not open");
    }
    await this.panel.webview.postMessage({
      action,
      type: "preview.__test.dom_action",
    });
  }

  // ── Private ──

  private postState(activeId: string): void {
    if (!this.panel) return;
    this.activeId = activeId;
    const idx = this.ids.indexOf(activeId);
    const activePic = idx >= 0 ? this.pictureMap.get(activeId) : null;
    if (!activePic && this.ids.length > 0) {
      // Fallback to first
      const firstId = this.ids[0];
      const firstPic = this.pictureMap.get(firstId);
      if (firstPic) {
        this.activeId = firstId;
        this.postStateToPanel(firstId, 1, this.ids.length, `Attached image 1`);
        return;      }
    }
    if (activePic) {
      this.postStateToPanel(activeId, idx + 1, this.ids.length, `Attached image ${idx + 1}`);
    }
  }

  private postStateToPanel(activeId: string, position: number, total: number, displayLabel: string): void {
    const source = this.sections.find(section => section.pictures.some(picture => picture.id === activeId));
    if (source?.label.startsWith("Tool images · ")) displayLabel = source.label;
    this.panel?.webview.postMessage({
      type: "preview.state",
      data: {
        sections: this.sections,
        activeId,
        displayLabel,
        position,
        total,
      },
    });
  }

  private async handleSave(attachmentId: string): Promise<void> {
    const pic = this.pictureMap.get(attachmentId);
    if (!pic) {
      this.sendSaveResult(false, "Image not found");
      return;
    }

    const defaultUri = vscode.Uri.file(path.join(vscode.workspace.rootPath ?? (process.env.HOME || "."), pic.filename));
    const uri = await vscode.window.showSaveDialog({
      defaultUri,
      filters: {
        Images: ["png", "jpg", "jpeg", "gif", "webp", "svg"],
      },
    });
    if (!uri) {
      this.sendSaveResult(false, undefined, undefined, true);
      return;    }

    try {
      // Copy straight from the blob store. Saving is the one operation that legitimately
      // needs the bytes in the host process, and it needs them exactly once, on demand.
      const source = this.blobUriFor(pic);
      if (!source) {
        this.sendSaveResult(false, "the image bytes are no longer available");
        return;
      }
      await vscode.workspace.fs.copy(source, uri, { overwrite: true });
      this.sendSaveResult(true, uri.fsPath);
    } catch (error) {
      this.sendSaveResult(false, String(error));
    }
  }

  /**
   * Recover the on-disk path behind a picture's webview URL.
   *
   * The hash is the last path segment of the URL, which is exactly what makes
   * content-addressed storage pleasant: the identifier and the filename are the same
   * thing, so there is no mapping table to keep in sync.
   */
  private blobUriFor(picture: PreviewPicture): vscode.Uri | null {
    if (!this.attachmentRoot) return null;
    const sha = picture.fullUri.split("/").pop()?.split("?")[0] ?? "";
    if (!/^[0-9a-f]{64}$/.test(sha)) return null;
    return vscode.Uri.file(path.join(this.attachmentRoot, "blobs", sha));
  }

  private sendSaveResult(
    success: boolean,
    savedPath?: string,
    error?: string,
    cancelled = false,
  ): void {
    this.panel?.webview.postMessage({
      type: "preview.saveResult",
      data: {
        cancelled,
        error: error ?? null,
        savedPath: savedPath ?? null,
        success,
      },
    });
  }
  private rejectTestSnapshotWaiters(error: Error): void {
    for (const waiter of this.testSnapshotWaiters.values()) {
      clearTimeout(waiter.timeout);
      waiter.reject(error);
    }
    this.testSnapshotWaiters.clear();
  }

  private buildHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.extensionUri.fsPath, "gui", "dist");
    const assets = resolveWebviewEntryAssets(distRoot, "image-preview.html", "image-preview.js");
    if (assets.scripts.length === 0) {
      return `<!DOCTYPE html><html><body><pre>Image preview assets not available. Run \`npm run build\`.</pre></body></html>`;
    }

    const nonce = getNonce();
    const styleTags = assets.stylesheets
      .map((file) => `<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}" />`)
      .join("\n    ");
    const scriptTags = assets.scripts
      .map((file) => `<script nonce="${nonce}" type="module" src="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}"></script>`)
      .join("\n    ");

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <meta http-equiv="Content-Security-Policy"
    content="default-src 'none'; img-src ${webview.cspSource} blob:; connect-src ${webview.cspSource}; font-src ${webview.cspSource}; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}' 'strict-dynamic';" />  ${styleTags}
  <title>Image Preview</title>
</head>
<body>
  <div id="root"></div>
  ${scriptTags}
</body>
</html>`;
  }

  /** Dispose resources when the extension shuts down. */
  dispose(): void {
    this.close();
  }
}

function getNonce(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let nonce = "";
  for (let index = 0; index < 32; index += 1) {
    nonce += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return nonce;
}
