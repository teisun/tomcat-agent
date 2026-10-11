import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { describe, expect, it } from "vitest";

import { withRichRenderFixtures } from "../src/test/suite/support/richRenderFixtures";

describe("rich-render Host fixture ownership", () => {
  it.each([false, true])("preserves local edits and cleans its own files (scenario failure=%s)", async (failScenario) => {
    const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-rich-render-fixture-test-"));
    const originalDirectory = path.join(workspace, "src", "test", "fixtures");
    const sentinel = "uncommitted local user changes\n";
    const originalFiles = ["rich-render.ts", "inline-link.ts"].map(name => path.join(originalDirectory, name));
    let ownedDirectory = "";
    const failure = new Error("scenario assertion failed");
    try {
      await fs.mkdir(originalDirectory, { recursive: true });
      for (const file of originalFiles) await fs.writeFile(file, sentinel);
      const scenario = withRichRenderFixtures(workspace, async (files) => {
        for (const file of originalFiles) expect(await fs.readFile(file, "utf8")).toBe(sentinel);
        ownedDirectory = path.dirname(files.richFilePath);
        expect(ownedDirectory).not.toBe(originalDirectory);
        expect(path.relative(workspace, ownedDirectory)).not.toMatch(/^\.\./);
        expect(path.dirname(files.inlineLinkPath)).toBe(ownedDirectory);
        expect((await fs.readFile(files.richFilePath, "utf8")).split("\n")[5]).toBe("  return 42;");
        expect(await fs.readFile(files.inlineLinkPath, "utf8")).toBe("export const inlineLink = true;\n");
        if (failScenario) throw failure;
      });
      if (failScenario) await expect(scenario).rejects.toBe(failure);
      else await scenario;
      for (const file of originalFiles) expect(await fs.readFile(file, "utf8")).toBe(sentinel);
      expect(ownedDirectory).not.toBe("");
      await expect(fs.access(ownedDirectory)).rejects.toMatchObject({ code: "ENOENT" });
    } finally {
      await fs.rm(workspace, { recursive: true, force: true });
    }
  });
});
