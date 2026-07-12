import { beforeEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { VsCodeIde } from "../VsCodeIde";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      deleteFile(filePath: string): void;
      lastDiffCommand?: { modified: vscode.Uri; original: vscode.Uri; title?: string };
      readFile(filePath: string): string | undefined;
      registerFile(filePath: string, text: string): void;
      reset(): void;
    };
  }
).__testing;

describe("VsCodeIde diff/apply", () => {
  beforeEach(() => {
    __testing.reset();
  });

  it("captures file snapshots and opens a virtual diff", async () => {
    __testing.registerFile("/workspace/src/example.ts", "before\n");
    const ide = new VsCodeIde();

    await ide.rememberToolStart("tool-1", { path: "src/example.ts" });
    __testing.registerFile("/workspace/src/example.ts", "after\n");
    await ide.rememberToolResult("tool-1", "src/example.ts");
    await ide.openPreparedDiff("tool-1");

    const diff = __testing.lastDiffCommand;
    expect(diff?.title).toContain("example.ts");

    const original = await vscode.workspace.openTextDocument(diff!.original);
    const proposed = await vscode.workspace.openTextDocument(diff!.modified);
    expect(original.getText()).toBe("before\n");
    expect(proposed.getText()).toBe("after\n");
  });

  it("applies prepared edits back into the workspace", async () => {
    const ide = new VsCodeIde();

    await ide.rememberToolStart("tool-2", { path: "src/new-file.ts" });
    __testing.registerFile("/workspace/src/new-file.ts", "hello from tomcat\n");
    await ide.rememberToolResult("tool-2", "src/new-file.ts");
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
