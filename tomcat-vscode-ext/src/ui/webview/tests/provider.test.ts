import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import {
  buildAttachmentOpenDialogOptions,
  classifyPickedUri,
  parseModelCatalog,
  parsePlanFrontmatter,
  readPlanMetadata,
} from "../provider";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerDirectory(dirPath: string): void;
      registerFile(filePath: string, text: string): void;
      reset(): void;
    };
  }
).__testing;

describe("plan metadata helpers", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(
      tempDirs.map(async (dir) => {
        await fs.rm(dir, { force: true, recursive: true });
      }),
    );
    tempDirs.length = 0;
  });

  it("parses title and overview from plan frontmatter", () => {
    const parsed = parsePlanFrontmatter(`---
name: Demo Plan UI
overview: Render the transcript UI with plan metadata.
todos:
  - id: one
---
# body
`);

    expect(parsed).toEqual({
      overview: "Render the transcript UI with plan metadata.",
      title: "Demo Plan UI",
    });
  });

  it("falls back to goal as title when name/title are absent", () => {
    const parsed = parsePlanFrontmatter(`---
goal: 在 test-stuff/ 下创建经典世嘉 OutRun 风格赛车网页游戏
draft: ...
---
# body
`);

    expect(parsed).toEqual({
      title: "在 test-stuff/ 下创建经典世嘉 OutRun 风格赛车网页游戏",
    });
  });

  it("truncates a long goal to the first line and 96 chars", () => {
    const longGoal = "目标".repeat(60);
    const parsed = parsePlanFrontmatter(`---
goal: ${longGoal}
---
`);
    expect(parsed.title).toBeDefined();
    expect(parsed.title!.length).toBeLessThanOrEqual(96);
    expect(parsed.title!.endsWith("...")).toBe(true);
  });

  it("prefers explicit title/name over goal", () => {
    const byTitle = parsePlanFrontmatter(`---
title: Explicit Title
goal: some goal
---
`);
    expect(byTitle.title).toBe("Explicit Title");

    const byName = parsePlanFrontmatter(`---
name: Named Plan
goal: some goal
---
`);
    expect(byName.title).toBe("Named Plan");
  });

  it("returns empty metadata when there is no frontmatter", () => {
    expect(parsePlanFrontmatter("# just a body\nno frontmatter here")).toEqual({});
  });

  it("reads metadata from disk and refreshes the cache when the file changes", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-plan-metadata-"));
    tempDirs.push(dir);
    const filePath = path.join(dir, "demo.plan.md");
    const cache = new Map<string, { mtimeMs: number; overview?: string; title?: string }>();

    await fs.writeFile(
      filePath,
      `---
name: First Title
overview: First overview.
---
`,
      "utf8",
    );

    const first = await readPlanMetadata(filePath, cache);
    expect(first).toEqual({
      overview: "First overview.",
      title: "First Title",
    });

    await new Promise((resolve) => setTimeout(resolve, 20));
    await fs.writeFile(
      filePath,
      `---
name: Updated Title
overview: Updated overview.
---
`,
      "utf8",
    );

    const second = await readPlanMetadata(filePath, cache);
    expect(second).toEqual({
      overview: "Updated overview.",
      title: "Updated Title",
    });
  });

  it("expands ~ in the plan path before reading from disk", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-plan-home-"));
    tempDirs.push(dir);
    const previousHome = process.env.HOME;
    process.env.HOME = dir;
    try {
      const planPath = path.join(dir, "demo.plan.md");
      await fs.writeFile(
        planPath,
        `---
goal: Home-expanded plan
---
`,
        "utf8",
      );

      const cache = new Map<string, { mtimeMs: number; overview?: string; title?: string }>();
      const metadata = await readPlanMetadata("~/demo.plan.md", cache);
      expect(metadata).toEqual({ title: "Home-expanded plan" });
    } finally {
      process.env.HOME = previousHome;
    }
  });
});

describe("attachment picker options", () => {
  it("allows any file or folder and updates the action label", () => {
    expect(buildAttachmentOpenDialogOptions()).toEqual({
      canSelectFiles: true,
      canSelectFolders: true,
      canSelectMany: true,
      openLabel: "Add to Tomcat",
    });
  });
});

describe("picked uri classification", () => {
  it("routes directories to references and images/pdf to attachments", async () => {
    __testing.reset();
    __testing.registerDirectory("/workspace/src/folder");
    __testing.registerFile("/workspace/assets/mockup.png", "png");
    __testing.registerFile("/workspace/specs/notes.pdf", "%PDF");
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");
    __testing.registerFile("/workspace/tmp/blob.bin", "raw");

    await expect(classifyPickedUri(vscode.Uri.file("/workspace/src/folder"))).resolves.toBe("reference");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/assets/mockup.png"))).resolves.toBe("attachment");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/specs/notes.pdf"))).resolves.toBe("attachment");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/src/app.ts"))).resolves.toBe("reference");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/tmp/blob.bin"))).resolves.toBe("reference");
  });
});

describe("model catalog parsing", () => {
  it("retains per-model capability metadata for the webview", () => {
    expect(
      parseModelCatalog({
        models: [
          {
            capabilities: {
              reasoning: true,
            },
            id: "deepseek-v4-flash",
            keyPresent: true,
          },
          {
            capabilities: ["vision", "files"],
            id: "gpt-5.4",
            keyPresent: true,
          },
          {
            capabilities: null,
            id: "text-only",
            keyPresent: true,
          },
          {
            capabilities: {
              tools: true,
            },
            id: "missing-key",
            keyPresent: false,
          },
        ],
      }),
    ).toEqual({
      capabilities: {
        "deepseek-v4-flash": ["reasoning"],
        "gpt-5.4": ["vision", "files"],
        "text-only": [],
      },
      ids: ["deepseek-v4-flash", "gpt-5.4", "text-only"],
    });
  });
});
