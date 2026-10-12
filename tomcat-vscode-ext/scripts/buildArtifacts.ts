import * as fs from "node:fs";
import * as path from "node:path";
import { createHash } from "node:crypto";

export const BUILD_RECORD = ".build-artifacts.json";
export const REQUIRED_OUTPUTS = ["out/extension.js", ...["index", "plan", "settings", "image-preview"].map((name) => `gui/dist/${name}.js`)];
const INPUTS = ["src", "scripts", "gui/src", "gui/public", "package.json", "package-lock.json", "tsconfig.json", "gui/package.json", "gui/package-lock.json", "gui/vite.config.ts", "gui/tsconfig.json", "gui/tsconfig.app.json", "gui/tsconfig.node.json", "gui/index.html", "gui/plan.html", "gui/settings.html", "gui/image-preview.html", "../tomcat/src", "../tomcat/Cargo.toml", "../tomcat/Cargo.lock"];

function snapshot(root: string, entries: readonly string[]): Record<string, string> {
  const files: Record<string, string> = {};
  function visit(relative: string): void {
    const absolute = path.join(root, relative);
    if (!fs.existsSync(absolute)) { files[relative] = "missing"; return; }
    const stat = fs.lstatSync(absolute);
    if (stat.isSymbolicLink()) throw new Error(`Build inputs/outputs must not be symlinks: ${relative}`);
    if (stat.isDirectory()) {
      for (const name of fs.readdirSync(absolute).sort()) {
        if (["node_modules", ".DS_Store"].includes(name)) continue;
        visit(`${relative}/${name}`);
      }
    } else if (stat.isFile()) {
      files[relative] = createHash("sha256").update(fs.readFileSync(absolute)).digest("hex");
    }
  }
  for (const entry of [...entries].sort()) visit(entry);
  return files;
}

function currentBuild(root: string) {
  for (const file of REQUIRED_OUTPUTS) {
    if (!fs.existsSync(path.join(root, file)) || fs.statSync(path.join(root, file)).size === 0) {
      throw new Error(`missing build artifact: ${file}; run npm run build`);
    }
  }
  return {
    version: 1,
    inputs: snapshot(root, INPUTS),
    outputs: snapshot(root, ["out", "gui/dist"]),
    // Vite substitutes this value into the emitted GUI.
    contextSearchDebounce: process.env.TOMCAT_CONTEXT_SEARCH_DEBOUNCE_MS ?? "",
  };
}

/** Called only after the complete wire/GUI/extension build succeeds. */
export function recordBuildArtifacts(root: string): void {
  fs.writeFileSync(path.join(root, BUILD_RECORD), JSON.stringify(currentBuild(root), null, 2) + "\n");
}

export function assertBuildArtifacts(root: string): void {
  let recorded: ReturnType<typeof currentBuild>;
  try { recorded = JSON.parse(fs.readFileSync(path.join(root, BUILD_RECORD), "utf8")); }
  catch { throw new Error("missing or malformed build record; run npm run build before --skip-build"); }
  const current = currentBuild(root);
  if (recorded.version !== 1 || JSON.stringify(recorded.inputs) !== JSON.stringify(current.inputs)
      || recorded.contextSearchDebounce !== current.contextSearchDebounce) {
    throw new Error("stale build inputs; run npm run build before --skip-build");
  }
  if (JSON.stringify(recorded.outputs) !== JSON.stringify(current.outputs)) {
    throw new Error("missing or corrupt build outputs; run npm run build before --skip-build");
  }
}

if (require.main === module) recordBuildArtifacts(path.resolve(__dirname, ".."));
