import { execFile } from "node:child_process";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "vitest";

const exec = promisify(execFile);
const root = path.resolve(__dirname, "..");
const cli = path.join(root, "node_modules/vitest/vitest.mjs");
const config = "vitest.integration.config.ts";

async function list(filters: string[] = []): Promise<string[]> {
  const temp = await mkdtemp(path.join(tmpdir(), "vitest-selection-"));
  const output = path.join(temp, "files.json");
  try {
    // --json accepts an optional *path*, including the literal string "true".
    // Use an explicitly owned output path, never a following source filename.
    await exec(process.execPath,
      [cli, "list", "--config", config, "--filesOnly", `--json=${output}`, ...filters],
      { cwd: root, timeout: 15_000, maxBuffer: 4 * 1024 * 1024 });
    return (JSON.parse(await readFile(output, "utf8")) as { file: string }[])
      .map(({ file }) => path.relative(root, file).replaceAll("\\", "/")).sort();
  } finally { await rm(temp, { recursive: true, force: true }); }
}

async function testFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const found = await Promise.all(entries.map(async (entry) => {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) return testFiles(file);
    return entry.isFile() && file.endsWith(".test.ts")
      ? [path.relative(root, file).replaceAll("\\", "/")] : [];
  }));
  return found.flat().sort();
}

describe("actual integration test selection", () => {
  it("lists every integration file and excludes src unit/Electron files", async () => {
    expect(await list()).toEqual(await testFiles(path.join(root, "tests")));
  }, 20_000);

  it("selects one or multiple requested files, without treating them as JSON output paths", async () => {
    const first = "tests/manifest_contract.test.ts";
    const second = "tests/webview_provider_flow.test.ts";
    const original = await readFile(path.join(root, first), "utf8");
    expect(await list([first])).toEqual([first]);
    expect(await list([first, second])).toEqual([first, second]);
    expect(await readFile(path.join(root, first), "utf8")).toBe(original);
  }, 35_000);

  it("fails the run when no requested test exists", async () => {
    await expect(exec(process.execPath,
      [cli, "run", "--config", config, "__no_such_integration_test__"],
      { cwd: root, timeout: 15_000 }))
      .rejects.toMatchObject({ code: 1 });
  }, 20_000);
});
