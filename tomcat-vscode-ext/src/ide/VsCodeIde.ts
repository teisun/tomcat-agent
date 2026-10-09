import * as os from "node:os";
import * as path from "node:path";

import * as vscode from "vscode";

import type { DiffFragment, DiffPresentation, DiffRange } from "../shared/diffPresentation";

const DIFF_SCHEME = "tomcat-diff";

type DiffSide = "original" | "proposed";

function formatDiffRange(range: DiffRange): string {
  return range.start === range.end ? `L${range.start}` : `L${range.start}-${range.end}`;
}

function fragmentFileName(fileName: string, fragment: DiffFragment, index: number, total: number): string {
  const extension = path.extname(fileName);
  const name = fileName.slice(0, fileName.length - extension.length);
  const ranges = [
    fragment.oldRange ? `原 ${formatDiffRange(fragment.oldRange)}` : "",
    fragment.newRange ? `新 ${formatDiffRange(fragment.newRange)}` : "",
  ].filter(Boolean).join(" → ");
  return `${name} · 片段 ${index + 1}／${total}${ranges ? ` · ${ranges}` : ""}${extension}`;
}

export class VsCodeIde implements vscode.TextDocumentContentProvider, vscode.Disposable {
  private readonly previewContents = new Map<string, string>();
  private readonly providerRegistration: vscode.Disposable;

  constructor() {
    this.providerRegistration = vscode.workspace.registerTextDocumentContentProvider(
      DIFF_SCHEME,
      this,
    );
  }

  dispose(): void {
    this.providerRegistration.dispose();
    this.previewContents.clear();
  }

  async openDiffPreview(
    sessionId: string,
    toolCallId: string,
    displayPath: string,
    presentation: DiffPresentation,
  ): Promise<void> {
    if (presentation.fragments.length === 0) {
      throw new Error("没有可查看的变更内容。");
    }

    const fileName = path.basename(displayPath);
    const pairs = presentation.fragments.map((fragment, index) => {
      const label = presentation.kind === "full"
        ? fileName
        : fragmentFileName(fileName, fragment, index, presentation.fragments.length);
      const createUri = (side: DiffSide, content: string): vscode.Uri => {
        const uri = vscode.Uri.from({
          scheme: DIFF_SCHEME,
          path: `/${label}`,
          query: new URLSearchParams({
            sessionId,
            toolCallId,
            fragment: String(index),
            side,
          }).toString(),
        });
        this.previewContents.set(uri.toString(), content);
        return uri;
      };
      const original = createUri("original", fragment.before);
      const proposed = createUri("proposed", fragment.after);
      return { original, proposed };
    });

    await this.ensureSideBySideDiffRendering();
    if (presentation.kind === "full") {
      const { original, proposed } = pairs[0];
      await vscode.commands.executeCommand(
        "vscode.diff",
        original,
        proposed,
        `${fileName}: Original ↔ Tomcat`,
        { preview: false },
      );
    } else {
      await vscode.commands.executeCommand(
        "vscode.changes",
        `${fileName} · 本次修改（${pairs.length} 个变更片段）`,
        pairs.map(({ original, proposed }): [vscode.Uri, vscode.Uri, vscode.Uri] => [
          proposed, original, proposed,
        ]),
      );
    }
  }

  async openSessionFileDiff(sessionId: string, sourceTurnId: string, filePath: string, before: string): Promise<void> {
    const fileName = path.basename(filePath);
    const original = vscode.Uri.from({ scheme: DIFF_SCHEME, path: `/${fileName}`, query: new URLSearchParams({ sessionId, sourceTurnId, path: filePath, side: "baseline" }).toString() });
    this.previewContents.set(original.toString(), before);
    let modified = vscode.Uri.file(filePath);
    try {
      const stat = await vscode.workspace.fs.stat(modified);
      if (stat.type & vscode.FileType.Directory) throw new Error("The changed path is a directory.");
    } catch (error) {
      if (!(error instanceof vscode.FileSystemError) || error.code !== "FileNotFound") throw error;
      modified = vscode.Uri.from({ scheme: DIFF_SCHEME, path: `/${fileName}`, query: new URLSearchParams({ sessionId, sourceTurnId, path: filePath, side: "missing" }).toString() });
      this.previewContents.set(modified.toString(), "");
    }
    await this.ensureSideBySideDiffRendering();
    await vscode.commands.executeCommand("vscode.diff", original, modified, `${fileName}: Review baseline ↔ Current`, { preview: false });
  }

  async showFile(displayPath: string, line?: number): Promise<void> {
    const uri = vscode.Uri.file(this.resolveWorkspacePath(displayPath));
    let stat: vscode.FileStat;
    try {
      stat = await vscode.workspace.fs.stat(uri);
    } catch (error) {
      if (!(error instanceof vscode.FileSystemError)) {
        throw error;
      }
      throw new Error(`File not found: ${uri.fsPath}`);
    }
    if (stat.type & vscode.FileType.Directory) {
      await vscode.commands.executeCommand("revealInExplorer", uri);
      return;
    }
    const document = await vscode.workspace.openTextDocument(uri);
    const editor = await vscode.window.showTextDocument(document, { preview: false });
    if (typeof line === "number" && Number.isFinite(line) && line > 0) {
      const targetLine = Math.min(Math.max(0, Math.trunc(line) - 1), Math.max(document.lineCount - 1, 0));
      const targetCharacter = document.lineAt(targetLine).text.length;
      const range = new vscode.Range(targetLine, 0, targetLine, targetCharacter);
      const selection = new vscode.Selection(
        new vscode.Position(targetLine, 0),
        new vscode.Position(targetLine, targetCharacter),
      );
      if ("selection" in editor) {
        editor.selection = selection;
      }
      if (vscode.window.activeTextEditor) {
        vscode.window.activeTextEditor.selection = selection;
      }
      if ("revealRange" in editor && typeof editor.revealRange === "function") {
        editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
      } else if (vscode.window.activeTextEditor?.revealRange) {
        vscode.window.activeTextEditor.revealRange(
          range,
          vscode.TextEditorRevealType.InCenterIfOutsideViewport,
        );
      }
    }
  }

  /**
   * Open a file with a specific custom editor (view type). Falls back to the
   * regular text editor when the custom editor cannot be resolved.
   */
  async openWith(displayPath: string, viewType: string): Promise<void> {
    const uri = vscode.Uri.file(this.resolveWorkspacePath(displayPath));
    if (!(await this.fileExists(uri))) {
      throw new Error(`File not found: ${uri.fsPath}`);
    }
    try {
      await vscode.commands.executeCommand("vscode.openWith", uri, viewType);
    } catch {
      const document = await vscode.workspace.openTextDocument(uri);
      await vscode.window.showTextDocument(document, { preview: false });
    }
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    const content = this.previewContents.get(uri.toString());
    if (content === undefined) {
      throw new Error("变更预览已不可用，请从会话中的 View diff 重新打开。");
    }
    return content;
  }

  private async fileExists(uri: vscode.Uri): Promise<boolean> {
    try {
      await vscode.workspace.fs.stat(uri);
      return true;
    } catch (error) {
      if (error instanceof vscode.FileSystemError) {
        return false;
      }
      throw error;
    }
  }

  private resolveWorkspacePath(filePath: string): string {
    if (path.isAbsolute(filePath)) {
      return filePath;
    }

    if (filePath.startsWith("~/")) {
      return path.join(os.homedir(), filePath.slice(2));
    }

    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (workspaceRoot) {
      return path.resolve(workspaceRoot, filePath);
    }

    return path.resolve(filePath);
  }

  private async ensureSideBySideDiffRendering(): Promise<void> {
    try {
      const diffEditorConfig = vscode.workspace.getConfiguration("diffEditor");
      const renderSideBySide = diffEditorConfig.inspect<boolean>("renderSideBySide");
      if (
        renderSideBySide?.globalValue === false
        || renderSideBySide?.workspaceValue === false
        || renderSideBySide?.workspaceFolderValue === false
      ) {
        return;
      }

      const inlineBreakpoint = diffEditorConfig.inspect<number>("renderSideBySideInlineBreakpoint");
      if (
        inlineBreakpoint?.globalValue !== undefined
        || inlineBreakpoint?.workspaceValue !== undefined
        || inlineBreakpoint?.workspaceFolderValue !== undefined
      ) {
        return;
      }

      await diffEditorConfig.update(
        "renderSideBySideInlineBreakpoint",
        0,
        vscode.ConfigurationTarget.Global,
      );
    } catch {
      // Never block diff open because a config write failed.
    }
  }
}
