import * as os from "node:os";
import * as path from "node:path";

import * as vscode from "vscode";

const DIFF_SCHEME = "tomcat-diff";

type DiffSide = "original" | "proposed";

interface PreparedFileChange {
  absolutePath: string;
  displayPath: string;
  existedBefore: boolean;
  originalContent: string;
  proposedContent: string;
  toolCallId: string;
}

interface PreparedSnapshot {
  absolutePath: string;
  displayPath: string;
  existedBefore: boolean;
  originalContent: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function getFilePathFromArgs(args: unknown): string | undefined {
  if (!isRecord(args) || typeof args.path !== "string") {
    return undefined;
  }

  return args.path;
}

function toSearchParams(side: DiffSide): string {
  return new URLSearchParams({ side }).toString();
}

export class VsCodeIde implements vscode.TextDocumentContentProvider, vscode.Disposable {
  private readonly preparedChanges = new Map<string, PreparedFileChange>();
  private readonly preparedSnapshots = new Map<string, PreparedSnapshot>();
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
    this.preparedSnapshots.clear();
  }

  async rememberToolStart(toolCallId: string, args: unknown): Promise<void> {
    const displayPath = getFilePathFromArgs(args);
    if (!displayPath) {
      return;
    }

    const absolutePath = this.resolveWorkspacePath(displayPath);
    const uri = vscode.Uri.file(absolutePath);
    const originalContent = await this.readFileIfExists(uri);

    this.preparedSnapshots.set(toolCallId, {
      absolutePath,
      displayPath,
      existedBefore: originalContent !== undefined,
      originalContent: originalContent ?? "",
    });
  }

  async rememberToolResult(toolCallId: string, displayPath: string): Promise<PreparedFileChange> {
    const snapshot =
      this.preparedSnapshots.get(toolCallId) ??
      ({
        absolutePath: this.resolveWorkspacePath(displayPath),
        displayPath,
        existedBefore: false,
        originalContent: "",
      } satisfies PreparedSnapshot);

    const proposedUri = vscode.Uri.file(snapshot.absolutePath);
    const proposedContent = (await this.readFileIfExists(proposedUri)) ?? "";

    const change: PreparedFileChange = {
      absolutePath: snapshot.absolutePath,
      displayPath: snapshot.displayPath,
      existedBefore: snapshot.existedBefore,
      originalContent: snapshot.originalContent,
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

  provideTextDocumentContent(uri: vscode.Uri): string {
    const side = new URLSearchParams(uri.query).get("side") as DiffSide | null;
    const change = this.preparedChanges.get(uri.authority);
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
      authority: toolCallId,
      path: `/${fileName}`,
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
}
