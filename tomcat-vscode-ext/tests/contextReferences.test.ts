import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";
import { __testing } from "vscode";

import {
  buildFileReference,
  buildSelectionReference,
  buildSelectionReferenceFromParts,
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

beforeEach(() => __testing.reset());
afterEach(() => __testing.reset());

describe("context references", () => {
  it("keeps multi-root file and selection targets distinct despite identical short labels", () => {
    (vscode.workspace as unknown as { workspaceFolders: Array<{ uri: vscode.Uri; name: string }> }).workspaceFolders = [
      { uri: vscode.Uri.file("/projects/remotecontrol_wins"), name: "Remote" },
      { uri: vscode.Uri.file("/projects/webrtc-sdk"), name: "WebRTC" },
    ];
    const paths = [
      "/projects/remotecontrol_wins/src/sunmi/webrtc_patch.h",
      "/projects/webrtc-sdk/src/sunmi/webrtc_patch.h",
    ];
    // Model the real VS Code API: false drops the root, default/true keeps its display name.
    expect(vscode.workspace.asRelativePath(vscode.Uri.file(paths[1]), false)).toBe("src/sunmi/webrtc_patch.h");
    expect(vscode.workspace.asRelativePath(vscode.Uri.file(paths[1]))).toBe("WebRTC/src/sunmi/webrtc_patch.h");
    expect(vscode.workspace.asRelativePath(vscode.Uri.file(paths[1]), true)).toBe("WebRTC/src/sunmi/webrtc_patch.h");
    for (const filePath of paths) {
      expect(buildFileReference(vscode.Uri.file(filePath))).toMatchObject({ path: filePath, label: "webrtc_patch.h" });
      const editor = createEditor(filePath, "selected", new vscode.Position(0, 0), new vscode.Position(0, 8));
      expect(buildSelectionReference(editor)).toMatchObject({ path: filePath, label: "webrtc_patch.h:1", text: "selected" });
      expect(buildSelectionReferenceFromParts(editor.document.uri, "preview", 3, 4)).toMatchObject({
        path: filePath, label: "webrtc_patch.h:3-4", text: "preview",
      });
      expect(buildFileReference(vscode.Uri.file(filePath.slice(0, filePath.lastIndexOf("/"))), { isDirectory: true }))
        .toMatchObject({ path: `${filePath.slice(0, filePath.lastIndexOf("/"))}/`, label: "sunmi/" });
    }
  });

  it("does not rebase references when the user overrides the session directory or closes the workspace", () => {
    const uri = vscode.Uri.file("/workspace/src/app.ts");
    __testing.setConfiguration("tomcat.session.defaultCwd", "/elsewhere");
    expect(buildFileReference(uri).path).toBe(uri.fsPath);
    (vscode.workspace as { workspaceFolders: unknown[] }).workspaceFolders = [];
    expect(vscode.workspace.asRelativePath(uri)).toBe(uri.fsPath);
    expect(buildFileReference(uri).path).toBe(uri.fsPath);
  });

  it.each([String.raw`D:\dev\webrtc-sdk\src\补丁 file.h`, String.raw`\\server\share\webrtc-sdk\src\补丁 file.h`])(
    "preserves the supplied Windows fsPath without URI encoding or losing its prefix: %s", (fsPath) => {
      // Input preservation only; real Windows URI conversion requires Windows Host acceptance.
      const uri = { fsPath, path: "/not-the-filesystem-path" } as vscode.Uri;
      expect(buildFileReference(uri)).toMatchObject({ path: fsPath, label: "补丁 file.h" });
      expect(buildSelectionReferenceFromParts(uri, "snapshot", 1, 1)).toMatchObject({ path: fsPath, label: "补丁 file.h:1" });
    },
  );

  it("keeps the native directory prefix and uses one trailing slash marker", () => {
    const directory = path.join(path.resolve("/workspace"), "nested");
    const uri = { fsPath: `${directory}${path.sep}` } as vscode.Uri;
    expect(buildFileReference(uri, { isDirectory: true })).toMatchObject({
      path: `${directory}/`, label: "nested/",
    });
    expect(buildFileReference(uri).path).toBe(uri.fsPath);
  });

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
      path: "/workspace/src/app.ts",
      text: "const beta = 2;\nconst gamma = alpha + beta;",
      type: "reference",
    });
  });

  it("builds a parts-based selection reference with a multi-line label", () => {
    const reference = buildSelectionReferenceFromParts(
      vscode.Uri.file("/workspace/plans/sample.plan.md"),
      "first line\nsecond line",
      12,
      14,
    );

    expect(reference).toEqual({
      kind: "selection",
      label: "sample.plan.md:12-14",
      lineEnd: 14,
      lineStart: 12,
      path: "/workspace/plans/sample.plan.md",
      text: "first line\nsecond line",
      type: "reference",
    });
  });

  it("uses a single-line label when start and end match", () => {
    const reference = buildSelectionReferenceFromParts(
      vscode.Uri.file("/workspace/plans/sample.plan.md"),
      "only line",
      7,
      7,
    );

    expect(reference?.label).toBe("sample.plan.md:7");
    expect(reference?.lineStart).toBe(7);
    expect(reference?.lineEnd).toBe(7);
  });

  it("omits line numbers (file-name label) when the range is unknown", () => {
    const reference = buildSelectionReferenceFromParts(
      vscode.Uri.file("/workspace/plans/sample.plan.md"),
      "orphan selection",
    );

    expect(reference).toEqual({
      kind: "selection",
      label: "sample.plan.md",
      lineEnd: null,
      lineStart: null,
      path: "/workspace/plans/sample.plan.md",
      text: "orphan selection",
      type: "reference",
    });
  });

  it("returns null when the selection text is empty", () => {
    expect(
      buildSelectionReferenceFromParts(vscode.Uri.file("/workspace/plans/sample.plan.md"), ""),
    ).toBeNull();
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

  it("builds in-workspace directory references with absolute paths and trailing slashes", () => {
    const reference = buildFileReference(vscode.Uri.file("/workspace/src/nested"), {
      isDirectory: true,
    });

    expect(reference).toEqual({
      kind: "file",
      label: "nested/",
      path: "/workspace/src/nested/",
      type: "reference",
    });
  });

  it("keeps absolute paths for files outside the workspace", () => {
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
      path: "/workspace/src/app.ts",
      type: "reference",
    });
    await expect(resolveUriToFileReference(vscode.Uri.file("/workspace/src/folder"))).resolves.toEqual({
      kind: "file",
      label: "folder/",
      path: "/workspace/src/folder/",
      type: "reference",
    });
  });
});
