import { describe, expect, it } from "vitest";
import * as vscode from "vscode";
import { __testing } from "vscode";

import {
  buildFileReference,
  buildSelectionReference,
  resolveUriToFileReference,
  truncateSelectionSnapshot,
} from "../src/ui/webview/contextReferences";

function offsetAt(text: string, position: vscode.Position): number {
  const lines = text.split("\n");
  let offset = 0;
  for (let line = 0; line < position.line; line += 1) {
    offset += (lines[line]?.length ?? 0) + 1;
  }
  return offset + position.character;
}

function positionAt(text: string, targetOffset: number): vscode.Position {
  const lines = text.split("\n");
  let remaining = Math.max(0, targetOffset);
  for (let line = 0; line < lines.length; line += 1) {
    const lineLength = lines[line]?.length ?? 0;
    if (remaining <= lineLength) {
      return new vscode.Position(line, remaining);
    }
    remaining -= lineLength + 1;
  }
  const lastLine = Math.max(0, lines.length - 1);
  return new vscode.Position(lastLine, lines[lastLine]?.length ?? 0);
}

function createEditor(
  filePath: string,
  text: string,
  start: vscode.Position,
  end: vscode.Position,
): vscode.TextEditor {
  const document = {
    getText(range?: { start: vscode.Position; end: vscode.Position }) {
      if (!range) {
        return text;
      }
      return text.slice(offsetAt(text, range.start), offsetAt(text, range.end));
    },
    offsetAt(position: vscode.Position) {
      return offsetAt(text, position);
    },
    positionAt(offset: number) {
      return positionAt(text, offset);
    },
    uri: vscode.Uri.file(filePath),
  } as unknown as vscode.TextDocument;

  return {
    document,
    selection: new vscode.Selection(start, end),
  } as unknown as vscode.TextEditor;
}

describe("context references", () => {
  it("truncates oversized selection snapshots with a visible suffix", () => {
    const input = `${"x".repeat(12_500)}\nsecond line`;

    const result = truncateSelectionSnapshot(input);

    expect(result.length).toBeLessThan(input.length);
    expect(result.endsWith("\n...[truncated by Tomcat]")).toBe(true);
  });

  it("builds selection references with 1-based inclusive line ranges", () => {
    const text = ["const alpha = 1;", "const beta = 2;", "const gamma = alpha + beta;", ""].join("\n");
    const editor = createEditor(
      "/workspace/src/app.ts",
      text,
      new vscode.Position(1, 0),
      new vscode.Position(2, "const gamma = alpha + beta;".length),
    );

    const reference = buildSelectionReference(editor);

    expect(reference).toEqual({
      kind: "selection",
      label: "app.ts:2-3",
      lineEnd: 3,
      lineStart: 2,
      path: "src/app.ts",
      text: "const beta = 2;\nconst gamma = alpha + beta;",
      type: "reference",
    });
  });

  it("truncates the selection snapshot before sending it to the webview", () => {
    const text = "y".repeat(12_500);
    const editor = createEditor(
      "/workspace/src/huge.ts",
      text,
      new vscode.Position(0, 0),
      new vscode.Position(0, text.length),
    );

    const reference = buildSelectionReference(editor);

    expect(reference?.text?.endsWith("\n...[truncated by Tomcat]")).toBe(true);
    expect(reference?.text?.length).toBeLessThan(text.length);
  });

  it("builds in-workspace directory references with relative paths and trailing slashes", () => {
    const reference = buildFileReference(vscode.Uri.file("/workspace/src/nested"), {
      isDirectory: true,
    });

    expect(reference).toEqual({
      kind: "file",
      label: "nested/",
      path: "src/nested/",
      type: "reference",
    });
  });

  it("falls back to absolute paths for files outside the workspace", () => {
    const reference = buildFileReference(vscode.Uri.file("/outside/log.txt"));

    expect(reference).toEqual({
      kind: "file",
      label: "log.txt",
      path: "/outside/log.txt",
      type: "reference",
    });
  });

  it("resolves file references the same way for command and drop paths", async () => {
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");
    __testing.registerDirectory("/workspace/src/folder");

    await expect(resolveUriToFileReference(vscode.Uri.file("/workspace/src/app.ts"))).resolves.toEqual({
      kind: "file",
      label: "app.ts",
      path: "src/app.ts",
      type: "reference",
    });
    await expect(resolveUriToFileReference(vscode.Uri.file("/workspace/src/folder"))).resolves.toEqual({
      kind: "file",
      label: "folder/",
      path: "src/folder/",
      type: "reference",
    });
  });
});
