import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

import {
  assertPublishableFiles,
  listPublishableFiles,
  packageVsix,
  preparePublishDirectory,
} from "../scripts/package-vsix";

describe("VSIX packaging", () => {
  it(
    "packages non-interactively and excludes source-only directories",
    async () => {
      const extensionRoot = path.resolve(__dirname, "..");
      const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-test-"));
      const vsixPath = path.join(tempRoot, "tomcat-vscode-ext.vsix");
      let publishRoot: string | undefined;

      try {
        const packaged = packageVsix({ extensionRoot, outPath: vsixPath });
        publishRoot = preparePublishDirectory(extensionRoot);
        const fileList = listPublishableFiles(publishRoot, extensionRoot);
        assertPublishableFiles(fileList);
        expect(fileList).toContain("README.md");
        expect(fileList).toContain("LICENSE");
        expect(fileList).toContain("gui/dist/index.js");
        expect(fileList).toContain("media/icon.png");
        expect(fileList).toContain("media/tomcat.svg");
        expect(fileList).not.toContain("src/extension.ts");
        expect(fileList).not.toContain("gui/src/App.tsx");
        expect(fileList).not.toContain("tests/serve_e2e.test.ts");

        const stat = await fs.stat(packaged);
        expect(stat.isFile()).toBe(true);
      } finally {
        if (publishRoot) {
          await fs.rm(publishRoot, { force: true, recursive: true });
        }
        await fs.rm(tempRoot, { force: true, recursive: true });
      }
    },
    180_000,
  );
});
