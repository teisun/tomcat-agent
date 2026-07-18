import { beforeEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { VsCodeIde } from "../VsCodeIde";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      deleteFile(filePath: string): void;
      lastDiffCommand?: { modified: vscode.Uri; original: vscode.Uri; title?: string };
      lastRevealRange?: { range: vscode.Range; revealType?: number };
      readFile(filePath: string): string | undefined;
      registerFile(filePath: string, text: string): void;
      reset(): void;
      setConfiguration(key: string, value: unknown): void;
    };
  }
).__testing;

describe("VsCodeIde diff/apply", () => {
  beforeEach(() => {
    __testing.reset();
  });

  it("captures structured diff pairs and opens a virtual diff", async () => {
    const ide = new VsCodeIde();

    __testing.registerFile("/workspace/src/example.ts", "after\n");
    await ide.rememberToolResult("tool-1", "src/example.ts", {
      after: "after\n",
      before: "before\n",
    });
    await ide.openPreparedDiff("tool-1");

    const diff = __testing.lastDiffCommand;
    expect(diff?.title).toContain("example.ts");

    const original = await vscode.workspace.openTextDocument(diff!.original);
    const proposed = await vscode.workspace.openTextDocument(diff!.modified);
    expect(original.getText()).toBe("before\n");
    expect(proposed.getText()).toBe("after\n");
    expect(
      vscode.workspace.getConfiguration("diffEditor").get<number>("renderSideBySideInlineBreakpoint"),
    ).toBe(0);
  });

  it("respects an explicit side-by-side disable when opening diffs", async () => {
    const ide = new VsCodeIde();
    __testing.setConfiguration("diffEditor.renderSideBySide", false);
    __testing.registerFile("/workspace/src/no-side-by-side.ts", "after\n");

    await ide.rememberToolResult("tool-no-side", "src/no-side-by-side.ts", {
      after: "after\n",
      before: "before\n",
    });
    await ide.openPreparedDiff("tool-no-side");

    expect(
      vscode.workspace.getConfiguration("diffEditor").get<number>("renderSideBySideInlineBreakpoint"),
    ).toBeUndefined();
  });

  it("does not overwrite an existing inline breakpoint", async () => {
    const ide = new VsCodeIde();
    __testing.setConfiguration("diffEditor.renderSideBySideInlineBreakpoint", 400);
    __testing.registerFile("/workspace/src/custom-breakpoint.ts", "after\n");

    await ide.rememberToolResult("tool-custom-breakpoint", "src/custom-breakpoint.ts", {
      after: "after\n",
      before: "before\n",
    });
    await ide.openPreparedDiff("tool-custom-breakpoint");

    expect(
      vscode.workspace.getConfiguration("diffEditor").get<number>("renderSideBySideInlineBreakpoint"),
    ).toBe(400);
  });

  it("marks oversize fallbacks as non-structured changes", async () => {
    const ide = new VsCodeIde();
    __testing.registerFile("/workspace/src/huge.ts", "current contents\n");

    const change = await ide.rememberToolResult("tool-huge", "src/huge.ts");

    expect(change.hasStructuredDiff).toBe(false);
    expect(change.originalContent).toBe("");
    expect(change.proposedContent).toBe("current contents\n");
  });

  it("applies prepared edits back into the workspace", async () => {
    const ide = new VsCodeIde();

    __testing.registerFile("/workspace/src/new-file.ts", "hello from tomcat\n");
    await ide.rememberToolResult("tool-2", "src/new-file.ts", {
      after: "hello from tomcat\n",
      before: "",
    });
    __testing.deleteFile("/workspace/src/new-file.ts");

    await expect(ide.applyPreparedEdit("tool-2")).resolves.toBe(true);
    expect(__testing.readFile("/workspace/src/new-file.ts")).toBe(
      "hello from tomcat\n",
    );
  });

  it("opens reconstructed diffs through the existing virtual document flow", async () => {
    const ide = new VsCodeIde();

    await ide.openReconstructedDiff(
      "tool-3",
      "src/reconstructed.ts",
      "before\nold line",
      "before\nnew line",
    );

    const diff = __testing.lastDiffCommand;
    expect(diff?.title).toContain("reconstructed.ts");

    const original = await vscode.workspace.openTextDocument(diff!.original);
    const proposed = await vscode.workspace.openTextDocument(diff!.modified);
    expect(original.getText()).toBe("before\nold line");
    expect(proposed.getText()).toBe("before\nnew line");
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

  it("preserves mixed-case tool ids when reconstructing diff documents", async () => {
    const ide = new VsCodeIde();

    await ide.openReconstructedDiff(
      "toolu_01AbC",
      "src/mixed-case.ts",
      "before\nleft side",
      "before\nright side",
    );

    const diff = __testing.lastDiffCommand;
    expect(diff?.title).toContain("mixed-case.ts");

    const original = await vscode.workspace.openTextDocument(diff!.original);
    const proposed = await vscode.workspace.openTextDocument(diff!.modified);
    expect(original.getText()).toBe("before\nleft side");
    expect(proposed.getText()).toBe("before\nright side");
  });
});
