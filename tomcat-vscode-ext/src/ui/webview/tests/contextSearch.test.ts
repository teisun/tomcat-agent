import { afterEach, describe, expect, it } from "vitest";
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

function resetContextSearchEnv(): void {
  delete process.env[CONTEXT_SEARCH_DISABLE_ENV];
  delete process.env[CONTEXT_SEARCH_LIMIT_ENV];
  delete process.env[CONTEXT_SEARCH_MAX_FILES_ENV];
}

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
      vscode.Uri.file("/workspace/src/components/Button.tsx"),
      vscode.Uri.file("/workspace/src/app.ts"),
      vscode.Uri.file("/workspace/docs/guide.md"),
    ]);

    expect(directories.map((uri) => vscode.workspace.asRelativePath(uri))).toEqual([
      "docs",
      "src",
      "src/components",
    ]);
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
          uri: vscode.Uri.file("/workspace/src/app.ts"),
        },
        {
          displayPath: "lib/app.ts",
          isDirectory: false,
          isOpen: true,
          label: "app.ts",
          uri: vscode.Uri.file("/workspace/lib/app.ts"),
        },
        {
          displayPath: "docs/application-notes.md",
          isDirectory: false,
          isOpen: false,
          label: "application-notes.md",
          uri: vscode.Uri.file("/workspace/docs/application-notes.md"),
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
  it("respects files.exclude when building the cache", async () => {
    __testing.registerFile("/workspace/generated/foo.ts", "export const generated = true;\n");
    __testing.registerFile("/workspace/src/foo.ts", "export const source = true;\n");
    __testing.setConfiguration("files.exclude", {
      "**/generated/**": true,
    });

    const service = new ContextSearchService();
    const result = await service.search({ query: "foo" });

    expect(result.matches.map((match) => match.reference.path)).toEqual(["src/foo.ts"]);
    service.dispose();
  });

  it("caps matches to the configured limit and marks the result as truncated", async () => {
    process.env[CONTEXT_SEARCH_LIMIT_ENV] = "2";
    __testing.registerFile("/workspace/src/app.ts", "export const app = true;\n");
    __testing.registerFile("/workspace/src/app.test.ts", "test('app', () => {});\n");
    __testing.registerFile("/workspace/src/application.ts", "export const application = true;\n");

    const service = new ContextSearchService();
    const result = await service.search({ query: "app" });

    expect(result.matches).toHaveLength(2);
    expect(result.truncated).toBe(true);
    service.dispose();
  });

  it("recomputes open-file weighting without rebuilding cached candidates", async () => {
    __testing.registerFile("/workspace/lib/app.ts", "export const libApp = true;\n");
    __testing.registerFile("/workspace/src/app.ts", "export const srcApp = true;\n");

    const service = new ContextSearchService();
    const firstResult = await service.search({ query: "app" });
    expect(firstResult.matches.map((match) => match.reference.path)).toEqual([
      "lib/app.ts",
      "src/app.ts",
    ]);

    const srcDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.file("/workspace/src/app.ts"),
    );
    await vscode.window.showTextDocument(srcDocument);

    const secondResult = await service.search({ query: "app" });
    expect(secondResult.matches.map((match) => match.reference.path)).toEqual([
      "src/app.ts",
      "lib/app.ts",
    ]);

    service.dispose();
  });

  it("invalidates the cache when the workspace watcher sees new files", async () => {
    __testing.registerFile("/workspace/src/one.ts", "export const one = 1;\n");
    const service = new ContextSearchService();

    await expect(service.search({ query: "two" })).resolves.toMatchObject({
      matches: [],
      truncated: false,
      workspaceAvailable: true,
    });

    __testing.registerFile("/workspace/src/two.ts", "export const two = 2;\n");

    await expect(service.search({ query: "two" })).resolves.toMatchObject({
      matches: [
        {
          description: "src",
          reference: {
            kind: "file",
            label: "two.ts",
            path: "src/two.ts",
            type: "reference",
          },
        },
      ],
      truncated: false,
      workspaceAvailable: true,
    });

    service.dispose();
  });

  it("does not surface empty directories as candidates", async () => {
    __testing.registerDirectory("/workspace/empty");
    __testing.registerFile("/workspace/src/app.ts", "export const app = true;\n");

    const service = new ContextSearchService();
    const result = await service.search({ query: "empty" });

    expect(result.matches).toEqual([]);
    service.dispose();
  });
});
