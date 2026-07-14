import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { resolveAllStylesheets, resolveWebviewEntryAssets } from "./guiAssets";

const tempDirs: string[] = [];

async function makeDist(files: Record<string, string>): Promise<string> {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-gui-assets-"));
  tempDirs.push(dir);
  await Promise.all(
    Object.entries(files).map(async ([relativePath, contents]) => {
      const filePath = path.join(dir, relativePath);
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      await fs.writeFile(filePath, contents, "utf8");
    }),
  );
  return dir;
}

afterEach(async () => {
  await Promise.all(tempDirs.map((dir) => fs.rm(dir, { force: true, recursive: true })));
  tempDirs.length = 0;
});

describe("resolveWebviewEntryAssets", () => {
  it("carries every stylesheet the built entry HTML declares (incl codicon.css)", async () => {
    const distRoot = await makeDist({
      "index.html": `<!doctype html><html><head>
        <script type="module" crossorigin src="./index.js"></script>
        <link rel="modulepreload" crossorigin href="./chunks/styles.js">
        <link rel="stylesheet" crossorigin href="./styles.css">
        <link rel="stylesheet" crossorigin href="./codicon.css">
      </head><body></body></html>`,
      "index.js": "console.log('index');",
      "styles.css": "body{}",
      "codicon.css": "@font-face{font-family:codicon}",
    });

    const assets = resolveWebviewEntryAssets(distRoot, "index.html", "index.js");

    expect(assets.scripts).toEqual([path.join(distRoot, "index.js")]);
    expect(assets.stylesheets.map((file) => path.basename(file))).toEqual([
      "styles.css",
      "codicon.css",
    ]);
  });

  it("ignores modulepreload links and non-module scripts", async () => {
    const distRoot = await makeDist({
      "settings.html": `<!doctype html><html><head>
        <script>var inline=1;</script>
        <script type="module" src="./settings.js"></script>
        <link rel="modulepreload" href="./chunks/x.js">
        <link rel="stylesheet" href="./styles.css">
      </head><body></body></html>`,
      "settings.js": "0;",
      "styles.css": "body{}",
    });

    const assets = resolveWebviewEntryAssets(distRoot, "settings.html", "settings.js");

    expect(assets.scripts).toEqual([path.join(distRoot, "settings.js")]);
    expect(assets.stylesheets.map((file) => path.basename(file))).toEqual(["styles.css"]);
  });

  it("skips references the build did not emit", async () => {
    const distRoot = await makeDist({
      "index.html": `<html><head>
        <script type="module" src="./index.js"></script>
        <link rel="stylesheet" href="./styles.css">
        <link rel="stylesheet" href="./missing.css">
      </head></html>`,
      "index.js": "0;",
      "styles.css": "body{}",
    });

    const assets = resolveWebviewEntryAssets(distRoot, "index.html", "index.js");

    expect(assets.stylesheets.map((file) => path.basename(file))).toEqual(["styles.css"]);
  });

  it("falls back to globbing all css when the entry HTML is missing", async () => {
    const distRoot = await makeDist({
      "index.js": "0;",
      "theme.css": "body{}",
      "codicon.css": "@font-face{font-family:codicon}",
    });

    const assets = resolveWebviewEntryAssets(distRoot, "index.html", "index.js");

    expect(assets.scripts).toEqual([path.join(distRoot, "index.js")]);
    // styles.css absent, so both remaining sheets are still carried.
    expect(assets.stylesheets.map((file) => path.basename(file)).sort()).toEqual([
      "codicon.css",
      "theme.css",
    ]);
  });

  it("reports no scripts when neither HTML nor the entry bundle exists", async () => {
    const distRoot = await makeDist({ "styles.css": "body{}" });

    const assets = resolveWebviewEntryAssets(distRoot, "index.html", "index.js");

    expect(assets.scripts).toEqual([]);
  });
});

describe("resolveAllStylesheets", () => {
  it("lists every css with styles.css first for deterministic ordering", async () => {
    const distRoot = await makeDist({
      "codicon.css": "a{}",
      "styles.css": "b{}",
      "extra.css": "c{}",
    });

    expect(resolveAllStylesheets(distRoot).map((file) => path.basename(file))).toEqual([
      "styles.css",
      "codicon.css",
      "extra.css",
    ]);
  });
});
