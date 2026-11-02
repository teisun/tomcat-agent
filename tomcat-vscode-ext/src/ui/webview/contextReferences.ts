import * as path from "node:path";

import * as vscode from "vscode";

import type { WebviewReference } from "./protocol";

const SELECTION_SNAPSHOT_CHAR_LIMIT = 12_000;
const SELECTION_TRUNCATION_SUFFIX = "\n...[truncated by Tomcat]";

function withDirectorySuffix(value: string, isDirectory: boolean): string {
  if (!isDirectory || value.endsWith("/")) {
    return value;
  }
  return `${value.endsWith(path.sep) ? value.slice(0, -1) : value}/`;
}

function basenameLabel(displayPath: string): string {
  const normalized = displayPath.replace(/\\/g, "/").replace(/\/+$/u, "");
  return normalized.split("/").at(-1) || displayPath;
}

function inclusiveSelectionEndLine(editor: vscode.TextEditor): number {
  const selection = editor.selection;
  const startOffset = editor.document.offsetAt(selection.start);
  const endOffset = editor.document.offsetAt(selection.end);
  if (endOffset <= startOffset) {
    return selection.start.line + 1;
  }
  return editor.document.positionAt(endOffset - 1).line + 1;
}

export function truncateSelectionSnapshot(text: string): string {
  if (text.length <= SELECTION_SNAPSHOT_CHAR_LIMIT) {
    return text;
  }
  const maxPrefixLength = Math.max(
    0,
    SELECTION_SNAPSHOT_CHAR_LIMIT - SELECTION_TRUNCATION_SUFFIX.length,
  );
  return `${text.slice(0, maxPrefixLength).trimEnd()}${SELECTION_TRUNCATION_SUFFIX}`;
}

/**
 * Assemble a `selection` reference from raw parts (not an editor). Shared by the
 * text-editor path (`buildSelectionReference`) and the plan preview webview,
 * whose selection lives in the DOM. Line numbers are optional: when they are
 * omitted (e.g. the rendered plan text could not be located in the source) the
 * label falls back to just the file name.
 */
export function buildSelectionReferenceFromParts(
  uri: vscode.Uri,
  rawText: string,
  lineStart?: number,
  lineEnd?: number,
): WebviewReference | null {
  if (!rawText) {
    return null;
  }
  // References may outlive the active editor or move to a session with another cwd.
  const filePath = uri.fsPath || uri.path;
  const base = basenameLabel(filePath);
  const text = truncateSelectionSnapshot(rawText);
  const hasLines = typeof lineStart === "number" && typeof lineEnd === "number";
  const label = hasLines
    ? lineStart === lineEnd
      ? `${base}:${lineStart}`
      : `${base}:${lineStart}-${lineEnd}`
    : base;
  return {
    kind: "selection",
    label,
    lineEnd: hasLines ? lineEnd : null,
    lineStart: hasLines ? lineStart : null,
    path: filePath,
    text,
    type: "reference",
  };
}

export function buildSelectionReference(
  editor: vscode.TextEditor,
): WebviewReference | null {
  const selection = editor.selection;
  if (selection.isEmpty) {
    return null;
  }
  const rawText = editor.document.getText(selection);
  if (!rawText) {
    return null;
  }
  return buildSelectionReferenceFromParts(
    editor.document.uri,
    rawText,
    selection.start.line + 1,
    inclusiveSelectionEndLine(editor),
  );
}

export function buildFileReference(
  uri: vscode.Uri,
  options: {
    isDirectory?: boolean;
  } = {},
): WebviewReference {
  const isDirectory = options.isDirectory === true;
  const filePath = withDirectorySuffix(uri.fsPath || uri.path, isDirectory);
  return {
    kind: "file",
    label: withDirectorySuffix(basenameLabel(filePath), isDirectory),
    path: filePath,
    type: "reference",
  };
}

export async function resolveUriToFileReference(
  uri: vscode.Uri,
): Promise<WebviewReference> {
  const stat = await vscode.workspace.fs.stat(uri).then(
    (value) => value,
    () => null,
  );
  return buildFileReference(uri, {
    isDirectory: stat?.type === vscode.FileType.Directory,
  });
}
