import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import {
  CONTEXT_SEARCH_DISABLE_ENV,
  CONTEXT_SEARCH_LIMIT_ENV,
  CONTEXT_SEARCH_MAX_FILES_ENV,
  ContextSearchService,
  deriveDirectories,
  fuzzyRank,
  readContextSearchConfig,
} from "../contextSearch";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerDirectory(dirPath: string): void;
      registerFile(filePath: string, text: string): void;
      reset(): void;
      setConfiguration(key: string, value: unknown): void;
    };
  }
).__testing;

// `/workspace` has no drive letter on Windows, so anchor it like a real fsPath.
const ROOT = path.resolve("/workspace");
const ws = (relative: string) => path.join(ROOT, relative);

function resetContextSearchEnv(): void {
  delete process.env[CONTEXT_SEARCH_DISABLE_ENV];
  delete process.env[CONTEXT_SEARCH_LIMIT_ENV];
  delete process.env[CONTEXT_SEARCH_MAX_FILES_ENV];
}

beforeEach(() => {
  (vscode.workspace as { workspaceFolders?: Array<{ uri: vscode.Uri }> }).workspaceFolders = [
    { uri: vscode.Uri.file(ROOT) },
  ];
});

afterEach(() => {
  resetContextSearchEnv();
  __testing.reset();
});

describe("context search config", () => {
  it("prefers env overrides over VS Code settings and defaults", () => {
    __testing.setConfiguration("tomcat.contextSearch.maxFiles", 321);
    process.env[CONTEXT_SEARCH_DISABLE_ENV] = "true";
    process.env[CONTEXT_SEARCH_LIMIT_ENV] = "7";
    process.env[CONTEXT_SEARCH_MAX_FILES_ENV] = "123";

    expect(readContextSearchConfig()).toEqual({
      disabled: true,
      limit: 7,
      maxFiles: 123,
    });
  });
});

describe("deriveDirectories", () => {
  it("derives unique parent directories and excludes empty folders", () => {
    const directories = deriveDirectories([
      vscode.Uri.file(ws("src/components/Button.tsx")),
      vscode.Uri.file(ws("src/app.ts")),
      vscode.Uri.file(ws("docs/guide.md")),
    ]);

    expect(directories.map((uri) => vscode.workspace.asRelativePath(uri))).toEqual([
      "docs",
      "src",
      "src/components",
    ]);
  });

  it("builds directory candidates with the same URI as Uri.file", () => {
    const [directory] = deriveDirectories([vscode.Uri.file(ws("src/app.ts"))]);

    expect(directory.toString()).toBe(vscode.Uri.file(ws("src")).toString());
  });
});

describe("fuzzyRank", () => {
  it("prefers basename hits, word boundaries, and open editors", () => {
    const ranked = fuzzyRank(
      [
        {
          displayPath: "src/app.ts",
          isDirectory: false,
          isOpen: false,
          label: "app.ts",
          uri: vscode.Uri.file(ws("src/app.ts")),
        },
        {
          displayPath: "lib/app.ts",
          isDirectory: false,
          isOpen: true,
          label: "app.ts",
          uri: vscode.Uri.file(ws("lib/app.ts")),
        },
        {
          displayPath: "docs/application-notes.md",
          isDirectory: false,
          isOpen: false,
          label: "application-notes.md",
          uri: vscode.Uri.file(ws("docs/application-notes.md")),
        },
      ],
      "app",
    );

    expect(ranked.map((candidate) => candidate.displayPath)).toEqual([
      "lib/app.ts",
      "src/app.ts",
      "docs/application-notes.md",
    ]);
  });
});

describe("ContextSearchService", () => {
  it("separates multi-root targets from display names and excludes absolute parent paths from ranking", async () => {
    let previousDescriptions: Array<string | null | undefined> | undefined;
    for (const parent of [ROOT, ws("unrelated-long-parent")]) {
      const roots = [path.join(parent, "remotecontrol_wins"), path.join(parent, "webrtc-sdk")];
      (vscode.workspace as unknown as { workspaceFolders: Array<{ uri: vscode.Uri; name: string }> }).workspaceFolders = [
        { uri: vscode.Uri.file(roots[0]), name: "Remote" },
        { uri: vscode.Uri.file(roots[1]), name: "WebRTC" },
      ];
      __testing.setConfiguration("files.exclude", { "generated/**": true });
      const targets = roots.map((root) => path.join(root, "src", "webrtc_patch.h"));
      targets.forEach((target) => __testing.registerFile(target, "content"));
      roots.forEach((root) => __testing.registerFile(path.join(root, "generated", "webrtc_patch.h"), "excluded"));
      const service = new ContextSearchService();
      try {
        const result = await service.search({ query: "webrtc_patch" });
        expect(result.matches).toHaveLength(2);
        expect(result.matches.map((match) => match.reference.path).sort()).toEqual([...targets].sort());
        expect(result.matches.map((match) => match.reference.label)).toEqual(["webrtc_patch.h", "webrtc_patch.h"]);
        const descriptions = result.matches.map((match) => match.description);
        expect([...descriptions].sort()).toEqual(["Remote/src", "WebRTC/src"]);
        if (previousDescriptions) expect(descriptions).toEqual(previousDescriptions);
        previousDescriptions = descriptions;
        const directories = (await service.search({ query: "src/" })).matches
          .filter((match) => match.reference.label.endsWith("/"));
        expect(directories.map((match) => match.reference.path).sort())
          .toEqual(roots.map((root) => `${path.join(root, "src")}/`).sort());
        expect(directories.map((match) => match.reference.label)).toEqual(["src/", "src/"]);
      } finally { service.dispose(); }
    }
  });

  it("resolves files, directories, and missing paths for Markdown chips", async () => {
    __testing.registerFile(ws("src/app.ts"), "export const app = true;\n");
    __testing.registerDirectory(ws("docs"));
    const service = new ContextSearchService();

    await expect(
      service.resolvePaths({ paths: ["src/app.ts", "docs", "missing.md"] }),
    ).resolves.toEqual([
      {
        kind: "file",
        path: "src/app.ts",
        resolvedPath: ws("src/app.ts"),
      },
      {
        kind: "directory",
        path: "docs",
        resolvedPath: ws("docs"),
      },
      {
        kind: "missing",
        path: "missing.md",
        resolvedPath: ws("missing.md"),
      },
    ]);
    service.dispose();
  });

  it("uses the warm @-mention index before falling back to stat", async () => {
    __testing.registerFile(ws("src/app.ts"), "export const app = true;\n");
    const service = new ContextSearchService();
    await service.search({ query: "app" });
    const stat = vi.spyOn(vscode.workspace.fs, "stat");

    await expect(service.resolvePaths({ paths: ["src/app.ts"] })).resolves.toMatchObject([
      { kind: "file", path: "src/app.ts" },
    ]);
    expect(stat).not.toHaveBeenCalled();
    stat.mockRestore();
    service.dispose();
  });

  it("invalidates direct path results when the workspace watcher sees a create", async () => {
    const service = new ContextSearchService();
    await expect(service.resolvePaths({ paths: ["src/new.ts"] })).resolves.toMatchObject([
      { kind: "missing", path: "src/new.ts" },
    ]);

    __testing.registerFile(ws("src/new.ts"), "export const fresh = true;\n");

    await expect(service.resolvePaths({ paths: ["src/new.ts"] })).resolves.toMatchObject([
      { kind: "file", path: "src/new.ts" },
    ]);
    service.dispose();
  });

  it("respects files.exclude when building the cache", async () => {
    __testing.registerFile(ws("generated/foo.ts"), "export const generated = true;\n");
    __testing.registerFile(ws("src/foo.ts"), "export const source = true;\n");
    __testing.setConfiguration("files.exclude", {
      "**/generated/**": true,
    });

    const service = new ContextSearchService();
    const result = await service.search({ query: "foo" });

    expect(result.matches.map((match) => match.reference.path)).toEqual([ws("src/foo.ts")]);
    service.dispose();
  });

  it("caps matches to the configured limit and marks the result as truncated", async () => {
    process.env[CONTEXT_SEARCH_LIMIT_ENV] = "2";
    __testing.registerFile(ws("src/app.ts"), "export const app = true;\n");
    __testing.registerFile(ws("src/app.test.ts"), "test('app', () => {});\n");
    __testing.registerFile(ws("src/application.ts"), "export const application = true;\n");

    const service = new ContextSearchService();
    const result = await service.search({ query: "app" });

    expect(result.matches).toHaveLength(2);
    expect(result.truncated).toBe(true);
    service.dispose();
  });

  it.each(["missing-directory/", "app"])("reports a saturated index even with few matches: %s", async query => {
    process.env[CONTEXT_SEARCH_MAX_FILES_ENV] = "2";
    __testing.registerFile(ws("src/app.ts"), "one");
    __testing.registerFile(ws("src/other.ts"), "two");
    __testing.registerFile(ws("missing-directory/nested.txt"), "not indexed");
    const service = new ContextSearchService();
    try {
      const result = await service.search({ query });
      expect(result.matches.length).toBeLessThan(20);
      expect(result.truncated).toBe(true);
    } finally { service.dispose(); }
  });

  it("recomputes open-file weighting without rebuilding cached candidates", async () => {
    __testing.registerFile(ws("lib/app.ts"), "export const libApp = true;\n");
    __testing.registerFile(ws("src/app.ts"), "export const srcApp = true;\n");

    const service = new ContextSearchService();
    const firstResult = await service.search({ query: "app" });
    expect(firstResult.matches.map((match) => match.reference.path)).toEqual([
      ws("lib/app.ts"),
      ws("src/app.ts"),
    ]);

    const srcDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.file(ws("src/app.ts")),
    );
    await vscode.window.showTextDocument(srcDocument);

    const secondResult = await service.search({ query: "app" });
    expect(secondResult.matches.map((match) => match.reference.path)).toEqual([
      ws("src/app.ts"),
      ws("lib/app.ts"),
    ]);

    service.dispose();
  });

  it("invalidates the cache when the workspace watcher sees new files", async () => {
    __testing.registerFile(ws("src/one.ts"), "export const one = 1;\n");
    const service = new ContextSearchService();

    await expect(service.search({ query: "two" })).resolves.toMatchObject({
      matches: [],
      truncated: false,
      workspaceAvailable: true,
    });

    __testing.registerFile(ws("src/two.ts"), "export const two = 2;\n");

    await expect(service.search({ query: "two" })).resolves.toMatchObject({
      matches: [
        {
          description: "src",
          reference: {
            kind: "file",
            label: "two.ts",
            path: ws("src/two.ts"),
            type: "reference",
          },
        },
      ],
      truncated: false,
      workspaceAvailable: true,
    });

    service.dispose();
  });

  it("does not clear a create invalidation that arrives during findFiles", async () => {
    __testing.registerFile(ws("src/one.ts"), "one");
    const service = new ContextSearchService();
    let finish!: (files: vscode.Uri[]) => void;
    const staleListing = new Promise<vscode.Uri[]>((resolve) => { finish = resolve; });
    const find = vi.spyOn(vscode.workspace, "findFiles").mockReturnValueOnce(staleListing);
    try {
      const searching = service.search({ query: "two" });
      __testing.registerFile(ws("src/two.ts"), "two");
      finish([vscode.Uri.file(ws("src/one.ts"))]);
      const result = await searching;
      expect(result.matches.map((match) => match.reference.path)).toContain(ws("src/two.ts"));
      await service.search({ query: "two" });
      expect(find).toHaveBeenCalledTimes(2);
    } finally { find.mockRestore(); service.dispose(); }
  });

  it("does not surface empty directories as candidates", async () => {
    __testing.registerDirectory(ws("empty"));
    __testing.registerFile(ws("src/app.ts"), "export const app = true;\n");

    const service = new ContextSearchService();
    const result = await service.search({ query: "empty" });

    expect(result.matches).toEqual([]);
    service.dispose();
  });
});
