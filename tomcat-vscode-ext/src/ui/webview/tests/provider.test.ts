import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({}));

import { parsePlanFrontmatter, readPlanMetadata } from "../provider";

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
});
