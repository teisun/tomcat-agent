import * as os from "node:os";
import * as path from "node:path";

import * as vscode from "vscode";

const DIFF_SCHEME = "tomcat-diff";

type DiffSide = "original" | "proposed";

interface PreparedFileChange {
  absolutePath: string;
  displayPath: string;
  existedBefore: boolean;
  hasStructuredDiff: boolean;
  originalContent: string;
  proposedContent: string;
  toolCallId: string;
}

interface FallbackDiffPair {
  after: string;
  before: string;
}

function toSearchParams(side: DiffSide): string {
  return new URLSearchParams({ side }).toString();
}

function encodeDiffPathSegment(value: string): string {
  return encodeURIComponent(value);
}

function createPreparedDiffPath(toolCallId: string, fileName: string): string {
  return `/${encodeDiffPathSegment(toolCallId)}/${encodeDiffPathSegment(fileName)}`;
}

function preparedDiffKeyFromPath(diffPath: string): string | null {
  const encodedKey = diffPath.split("/").filter(Boolean)[0];
  if (!encodedKey) {
    return null;
  }
  try {
    return decodeURIComponent(encodedKey);
  } catch {
    return null;
  }
}

export class VsCodeIde implements vscode.TextDocumentContentProvider, vscode.Disposable {
  private readonly preparedChanges = new Map<string, PreparedFileChange>();
  private readonly providerRegistration: vscode.Disposable;

  constructor() {
    this.providerRegistration = vscode.workspace.registerTextDocumentContentProvider(
      DIFF_SCHEME,
      this,
    );
  }

  dispose(): void {
    this.providerRegistration.dispose();
    this.preparedChanges.clear();
  }

  async rememberToolStart(_toolCallId: string, _args: unknown): Promise<void> {
    // Structured diffs from Rust are the only trustworthy source of "before".
  }

  async rememberToolResult(
    toolCallId: string,
    displayPath: string,
    fallbackDiff?: FallbackDiffPair,
  ): Promise<PreparedFileChange> {
    const absolutePath = this.resolveWorkspacePath(displayPath);
    const proposedUri = vscode.Uri.file(absolutePath);
    const proposedContent = (await this.readFileIfExists(proposedUri)) ?? fallbackDiff?.after ?? "";

    const change: PreparedFileChange = {
      absolutePath,
      displayPath,
      existedBefore: fallbackDiff ? fallbackDiff.before.length > 0 : proposedContent.length > 0,
      hasStructuredDiff: Boolean(fallbackDiff),
      originalContent: fallbackDiff?.before ?? "",
      proposedContent,
      toolCallId,
    };
    this.preparedChanges.set(toolCallId, change);
    return change;
  }

  getPreparedChange(toolCallId: string): PreparedFileChange | undefined {
    return this.preparedChanges.get(toolCallId);
  }

  createFileAnchor(displayPath: string): vscode.Uri {
    return vscode.Uri.file(this.resolveWorkspacePath(displayPath));
  }

  async openPreparedDiff(toolCallId: string): Promise<void> {
    const change = this.requirePreparedChange(toolCallId);
    const title = `${path.basename(change.absolutePath)}: Original ↔ Tomcat`;
    const originalUri = this.createPreparedDiffUri(toolCallId, path.basename(change.absolutePath), "original");
    const proposedUri = this.createPreparedDiffUri(toolCallId, path.basename(change.absolutePath), "proposed");
    await this.ensureSideBySideDiffRendering();

    await vscode.commands.executeCommand(
      "vscode.diff",
      originalUri,
      proposedUri,
      title,
      { preview: false },
    );
  }

  async openReconstructedDiff(
    toolCallId: string,
    displayPath: string,
    before: string,
    after: string,
  ): Promise<void> {
    const absolutePath = this.resolveWorkspacePath(displayPath);
    this.preparedChanges.set(toolCallId, {
      absolutePath,
      displayPath,
      existedBefore: before.length > 0,
      hasStructuredDiff: true,
      originalContent: before,
      proposedContent: after,
      toolCallId,
    });
    await this.openPreparedDiff(toolCallId);
  }

  async applyPreparedEdit(toolCallId: string): Promise<boolean> {
    const change = this.requirePreparedChange(toolCallId);
    const targetUri = vscode.Uri.file(change.absolutePath);
    const edit = new vscode.WorkspaceEdit();

    if (await this.fileExists(targetUri)) {
      const document = await vscode.workspace.openTextDocument(targetUri);
      const endLine = Math.max(document.lineCount - 1, 0);
      const endCharacter = document.lineAt(endLine).text.length;
      edit.replace(targetUri, new vscode.Range(0, 0, endLine, endCharacter), change.proposedContent);
    } else {
      edit.createFile(targetUri, { ignoreIfExists: true, overwrite: true });
      edit.insert(targetUri, new vscode.Position(0, 0), change.proposedContent);
    }

    const applied = await vscode.workspace.applyEdit(edit);
    if (!applied) {
      return false;
    }

    const document = await vscode.workspace.openTextDocument(targetUri);
    await document.save();
    await vscode.window.showTextDocument(document, { preview: false });
    return true;
  }

  async showFile(displayPath: string): Promise<void> {
    const uri = vscode.Uri.file(this.resolveWorkspacePath(displayPath));
    if (!(await this.fileExists(uri))) {
      throw new Error(`File not found: ${uri.fsPath}`);
    }
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document, { preview: false });
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
    const side = new URLSearchParams(uri.query).get("side") as DiffSide | null;
    const changeKey = preparedDiffKeyFromPath(uri.path);
    const change = changeKey ? this.preparedChanges.get(changeKey) : undefined;
    if (!change || !side) {
      return "";
    }

    return side === "original" ? change.originalContent : change.proposedContent;
  }

  private requirePreparedChange(toolCallId: string): PreparedFileChange {
    const change = this.preparedChanges.get(toolCallId);
    if (!change) {
      throw new Error(`No prepared file change found for ${toolCallId}`);
    }
    return change;
  }

  private createPreparedDiffUri(
    toolCallId: string,
    fileName: string,
    side: DiffSide,
  ): vscode.Uri {
    return vscode.Uri.from({
      scheme: DIFF_SCHEME,
      path: createPreparedDiffPath(toolCallId, fileName),
      query: toSearchParams(side),
    });
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

  private async readFileIfExists(uri: vscode.Uri): Promise<string | undefined> {
    try {
      const bytes = await vscode.workspace.fs.readFile(uri);
      return new TextDecoder().decode(bytes);
    } catch (error) {
      if (error instanceof vscode.FileSystemError) {
        return undefined;
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
