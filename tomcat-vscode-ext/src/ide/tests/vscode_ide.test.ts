import { beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import type { DiffPresentation } from "../../shared/diffPresentation";
import { VsCodeIde } from "../VsCodeIde";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerDirectory(dirPath: string): void;
      lastDiffCommand?: { modified: vscode.Uri; original: vscode.Uri; title?: string; options?: unknown };
      lastChangesCommand?: { title: string; changes: [vscode.Uri, vscode.Uri, vscode.Uri][] };
      lastRevealRange?: { range: vscode.Range; revealType?: number };
      registerFile(filePath: string, text: string): void;
      reset(): void;
      setConfiguration(key: string, value: unknown): void;
    };
  }
).__testing;

describe("VsCodeIde files", () => {
  beforeEach(() => {
    __testing.reset();
  });

  it("does not overwrite an existing inline breakpoint", async () => {
    const ide = new VsCodeIde();
    __testing.setConfiguration("diffEditor.renderSideBySideInlineBreakpoint", 400);
    await ide.openDiffPreview("session-1", "tool-custom-breakpoint", "src/custom-breakpoint.ts", {
      kind: "full",
      fragments: [{ before: "before\n", after: "after\n" }],
    });

    expect(
      vscode.workspace.getConfiguration("diffEditor").get<number>("renderSideBySideInlineBreakpoint"),
    ).toBe(400);
  });

  it("opens files at a specific line and reveals the selection", async () => {
    const ide = new VsCodeIde();
    __testing.registerFile("/workspace/src/reveal.ts", "alpha\nbeta\ngamma\n");

    await ide.showFile("src/reveal.ts", 2);

    expect(vscode.window.activeTextEditor?.document.uri.fsPath).toBe("/workspace/src/reveal.ts");
    expect(vscode.window.activeTextEditor?.selection.start.line).toBe(1);
    expect(vscode.window.activeTextEditor?.selection.end.line).toBe(1);
    expect(__testing.lastRevealRange?.range.start.line).toBe(1);
  });

  it("reveals a resolved directory in the explorer instead of opening it as text", async () => {
    const ide = new VsCodeIde();
    __testing.registerDirectory("/workspace/src/components");
    const executeCommand = vi.spyOn(vscode.commands, "executeCommand").mockResolvedValue(undefined);

    await ide.showFile("src/components");

    expect(executeCommand).toHaveBeenCalledWith(
      "revealInExplorer",
      vscode.Uri.file("/workspace/src/components"),
    );
    executeCommand.mockRestore();
  });
});

describe("VsCodeIde read-only history preview", () => {
  beforeEach(() => {
    __testing.reset();
  });

  async function readPair(original: vscode.Uri, proposed: vscode.Uri): Promise<string[]> {
    return Promise.all([original, proposed].map(async (uri) => (
      await vscode.workspace.openTextDocument(uri)
    ).getText()));
  }

  it("opens saved full contents without reading disk", async () => {
    const ide = new VsCodeIde();
    __testing.registerFile("/workspace/src/example.ts", "newer disk contents");
    const readFile = vi.spyOn(vscode.workspace.fs, "readFile");
    const stat = vi.spyOn(vscode.workspace.fs, "stat");

    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", {
      kind: "full",
      fragments: [{ before: "old\n", after: "new\n" }],
    });

    const diff = __testing.lastDiffCommand!;
    expect(diff.title).toBe("example.ts: Original ↔ Tomcat");
    expect(diff.options).toEqual({ preview: false });
    expect(diff.original.path).toBe("/example.ts");
    expect(diff.modified.path).toBe(diff.original.path);
    expect(diff.modified.query).not.toBe(diff.original.query);
    expect(await readPair(diff.original, diff.modified)).toEqual(["old\n", "new\n"]);
    expect(readFile).not.toHaveBeenCalled();
    expect(stat).not.toHaveBeenCalled();
    expect(__testing.lastChangesCommand).toBeUndefined();
    expect(vscode.workspace.getConfiguration("diffEditor").get("renderSideBySideInlineBreakpoint")).toBe(0);
  });

  it("opens all fragments in one native changes page with distinct source ranges", async () => {
    const ide = new VsCodeIde();
    const executeCommand = vi.spyOn(vscode.commands, "executeCommand");
    const presentation: DiffPresentation = {
      kind: "fragments",
      fragments: [
        { before: "alpha\nold", after: "alpha\nnew", oldRange: { start: 25, end: 26 }, newRange: { start: 25, end: 26 } },
        { before: "new\nbeta", after: "old\nbeta", oldRange: { start: 235, end: 236 }, newRange: { start: 235, end: 236 } },
      ],
    };

    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", presentation);

    const changes = __testing.lastChangesCommand!;
    expect(executeCommand).toHaveBeenCalledExactlyOnceWith("vscode.changes", changes.title, changes.changes);
    expect(changes.title).toBe("example.ts · 本次修改（2 个变更片段）");
    expect(changes.changes).toHaveLength(2);
    for (const [index, [resource, original, proposed]] of changes.changes.entries()) {
      expect(resource).toEqual(proposed);
      expect(original.path).toBe(proposed.path);
      expect(original.path).toContain(`片段 ${index + 1}／2`);
      expect(original.path).toMatch(/\.ts$/);
      expect(original.path).not.toContain("session-1");
      expect(original.path).not.toContain("tool-1");
      expect(original.query).not.toBe(proposed.query);
      expect(Object.fromEntries(new URLSearchParams(original.query))).toEqual({
        sessionId: "session-1", toolCallId: "tool-1", fragment: String(index), side: "original",
      });
      expect(new URLSearchParams(proposed.query).get("side")).toBe("proposed");
      expect(await readPair(original, proposed)).toEqual([
        presentation.fragments[index].before,
        presentation.fragments[index].after,
      ]);
    }
    expect(changes.changes[0][1].path).toContain("原 L25-26 → 新 L25-26");
    expect(changes.changes[1][1].path).toContain("原 L235-236 → 新 L235-236");
    expect(__testing.lastDiffCommand).toBeUndefined();
  });

  it.each(["full", "fragments"] as const)("isolates reused tool IDs between sessions for %s previews", async (kind) => {
    const ide = new VsCodeIde();
    const toolCallId = "toolu_01AbC/with?reserved=chars";
    const pairs: [vscode.Uri, vscode.Uri][] = [];
    for (const sessionId of ["session-A/with?reserved=chars", "session-B"]) {
      await ide.openDiffPreview(sessionId, toolCallId, "src/example.ts", {
        kind,
        fragments: [{ before: `${sessionId}: before`, after: `${sessionId}: after` }],
      });
      pairs.push(kind === "full"
        ? [__testing.lastDiffCommand!.original, __testing.lastDiffCommand!.modified]
        : [__testing.lastChangesCommand!.changes[0][1], __testing.lastChangesCommand!.changes[0][2]]);
    }

    expect(pairs[0][0].path).toBe(pairs[1][0].path);
    expect(pairs[0][0].toString()).not.toBe(pairs[1][0].toString());
    expect(await readPair(...pairs[0])).toEqual([
      "session-A/with?reserved=chars: before", "session-A/with?reserved=chars: after",
    ]);
    expect(await readPair(...pairs[1])).toEqual(["session-B: before", "session-B: after"]);
    expect(new URLSearchParams(pairs[0][0].query).get("toolCallId")).toBe(toolCallId);
  });

  it("keeps both read-only sides for added or deleted fragments and labels missing positions honestly", async () => {
    const ide = new VsCodeIde();
    await ide.openDiffPreview("session-1", "tool-1", "new.ts", {
      kind: "fragments",
      fragments: [
        { before: "", after: "added", newRange: { start: 4, end: 4 } },
        { before: "deleted", after: "", oldRange: { start: 10, end: 10 } },
        { before: "legacy old", after: "legacy new" },
      ],
    });

    const changes = __testing.lastChangesCommand!.changes;
    expect(await readPair(changes[0][1], changes[0][2])).toEqual(["", "added"]);
    expect(await readPair(changes[1][1], changes[1][2])).toEqual(["deleted", ""]);
    expect(changes[0][1].path).toBe("/new · 片段 1／3 · 新 L4.ts");
    expect(changes[1][1].path).toBe("/new · 片段 2／3 · 原 L10.ts");
    expect(changes[2][1].path).toBe("/new · 片段 3／3.ts");
  });

  it.each(["full", "fragments"] as const)("reuses stable %s preview URIs after the disk file changes", async (kind) => {
    const ide = new VsCodeIde();
    const presentation: DiffPresentation = { kind, fragments: [{ before: "before", after: "after" }] };
    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", presentation);
    const first = kind === "full" ? __testing.lastDiffCommand!.modified : __testing.lastChangesCommand!.changes[0][2];
    __testing.registerFile("/workspace/src/example.ts", "unrelated new contents");

    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", presentation);
    const second = kind === "full" ? __testing.lastDiffCommand!.modified : __testing.lastChangesCommand!.changes[0][2];

    expect(second.toString()).toBe(first.toString());
    expect((await vscode.workspace.openTextDocument(first)).getText()).toBe("after");
  });

  it("reports missing preview documents and clears their contents on disposal", async () => {
    const ide = new VsCodeIde();
    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", {
      kind: "full", fragments: [{ before: "old", after: "new" }],
    });
    const original = __testing.lastDiffCommand!.original;
    const unknown = original.with({ query: `${original.query}&unknown=1` });
    expect(() => ide.provideTextDocumentContent(unknown)).toThrow("变更预览已不可用");

    ide.dispose();

    expect(() => ide.provideTextDocumentContent(original)).toThrow("变更预览已不可用");
  });

  it("rejects an empty preview without opening an editor", async () => {
    const ide = new VsCodeIde();
    const executeCommand = vi.spyOn(vscode.commands, "executeCommand");
    await expect(ide.openDiffPreview("session-1", "tool-1", "src/example.ts", {
      kind: "fragments", fragments: [],
    })).rejects.toThrow("没有可查看的变更内容");
    expect(executeCommand).not.toHaveBeenCalled();
  });

  it.each(["full", "fragments"] as const)("respects explicit inline rendering for %s previews", async (kind) => {
    const ide = new VsCodeIde();
    __testing.setConfiguration("diffEditor.renderSideBySide", false);
    await ide.openDiffPreview("session-1", "tool-1", "src/example.ts", {
      kind, fragments: [{ before: "old", after: "new" }],
    });
    expect(vscode.workspace.getConfiguration("diffEditor").get("renderSideBySideInlineBreakpoint")).toBeUndefined();
  });
});
