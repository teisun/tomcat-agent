import * as fs from "node:fs/promises";
import * as path from "node:path";

export interface RichRenderFixturePaths {
  richFilePath: string;
  inlineLinkPath: string;
}

/** Keep the filesystem setup shared by the real Host flow and its safety test. */
export async function withRichRenderFixtures(
  workspaceRoot: string,
  run: (files: RichRenderFixturePaths) => Promise<void>,
): Promise<void> {
  const directory = await fs.mkdtemp(path.join(workspaceRoot, ".tomcat-rich-render-"));
  const richFilePath = path.join(directory, "rich-render.ts");
  const inlineLinkPath = path.join(directory, "inline-link.ts");
  try {
    await fs.writeFile(richFilePath, [
      "export function richRenderFixture() {",
      '  return "line two";',
      "}",
      "",
      "export function otherLine() {",
      "  return 42;",
      "}",
      "",
    ].join("\n"), "utf8");
    await fs.writeFile(inlineLinkPath, "export const inlineLink = true;\n", "utf8");
    await run({ richFilePath, inlineLinkPath });
  } finally {
    // Only the unique directory created by this call is ours to remove.
    await fs.rm(directory, { recursive: true, force: true });
  }
}
