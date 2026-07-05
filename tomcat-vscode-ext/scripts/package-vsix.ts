import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

type PackageManifest = {
  name: string;
  version: string;
};

export interface PackageVsixOptions {
  bundleBinaryPath?: string;
  extensionRoot?: string;
  outPath?: string;
  skipBuild?: boolean;
  target?: string;
}

const REQUIRED_FILES = [
  "LICENSE",
  "README.md",
  "package.json",
  "gui/dist/index.js",
  "media/icon.png",
  "media/tomcat.svg",
  "out/extension.js",
] as const;

const DISALLOWED_PREFIXES = [
  ".vscode-test/",
  "e2e-harness/",
  "gui/node_modules/",
  "gui/src/",
  "node_modules/",
  "out/test/",
  "out/tests/",
  "scripts/",
  "src/",
  "tests/",
] as const;

const ROOT_ASSETS = [
  ".vscodeignore",
  "CHANGELOG.md",
  "LICENSE",
  "README.md",
  "package.json",
] as const;

const DIRECTORY_ASSETS = [
  "gui/dist",
  "media",
] as const;

function run(command: string, args: string[], cwd: string, capture = false): string {
  const result = execFileSync(command, args, {
    cwd,
    encoding: "utf8",
    stdio: capture ? ["inherit", "pipe", "inherit"] : "inherit",
  });
  return capture ? result : "";
}

function resolveVsceBinary(extensionRoot: string): string {
  return path.join(
    extensionRoot,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "vsce.cmd" : "vsce",
  );
}

function runVsce(
  args: string[],
  cwd: string,
  extensionRoot: string,
  capture = false,
): string {
  return run(resolveVsceBinary(extensionRoot), args, cwd, capture);
}

function hasPath(fileList: readonly string[], expected: string): boolean {
  return fileList.includes(expected);
}

function hasPrefix(fileList: readonly string[], prefix: string): boolean {
  return fileList.some((file) => file === prefix.slice(0, -1) || file.startsWith(prefix));
}

function readManifest(extensionRoot: string): PackageManifest {
  return JSON.parse(
    fs.readFileSync(path.join(extensionRoot, "package.json"), "utf8"),
  ) as PackageManifest;
}

export function bundledExecutableRelativePath(target?: string): string {
  return target?.startsWith("win") ? "bin/tomcat.exe" : "bin/tomcat";
}

export function buildVsixOutPath(
  extensionRoot: string,
  manifest: PackageManifest,
  target?: string,
): string {
  const suffix = target ? `-${target}` : "";
  return path.join(extensionRoot, `${manifest.name}-${manifest.version}${suffix}.vsix`);
}

export function buildVscePackageArgs(outPath: string, target?: string): string[] {
  const args = ["package", "--no-dependencies"];
  if (target) {
    args.push("--target", target);
  }
  args.push("--out", outPath);
  return args;
}

function shouldIncludeOutPath(relativePath: string): boolean {
  if (
    relativePath === "test" ||
    relativePath === "tests" ||
    relativePath.startsWith("test/") ||
    relativePath.startsWith("tests/")
  ) {
    return false;
  }
  if (/(^|\/)tests(\/|$)/.test(relativePath)) {
    return false;
  }
  if (/\.test\.js(\.map)?$/.test(relativePath)) {
    return false;
  }
  return true;
}

function copyFilteredOut(sourceRoot: string, targetRoot: string, relativePath = ""): void {
  const sourceDir = path.join(sourceRoot, relativePath);
  for (const entry of fs.readdirSync(sourceDir, { withFileTypes: true })) {
    const childRelativePath = relativePath
      ? path.posix.join(relativePath, entry.name)
      : entry.name;
    if (!shouldIncludeOutPath(childRelativePath)) {
      continue;
    }

    const sourcePath = path.join(sourceRoot, childRelativePath);
    const targetPath = path.join(targetRoot, childRelativePath);
    if (entry.isDirectory()) {
      fs.mkdirSync(targetPath, { recursive: true });
      copyFilteredOut(sourceRoot, targetRoot, childRelativePath);
      continue;
    }

    fs.mkdirSync(path.dirname(targetPath), { recursive: true });
    fs.copyFileSync(sourcePath, targetPath);
  }
}

function copyDirectory(sourcePath: string, targetPath: string): void {
  fs.mkdirSync(targetPath, { recursive: true });
  for (const entry of fs.readdirSync(sourcePath, { withFileTypes: true })) {
    const sourceChild = path.join(sourcePath, entry.name);
    const targetChild = path.join(targetPath, entry.name);
    if (entry.isDirectory()) {
      copyDirectory(sourceChild, targetChild);
      continue;
    }
    fs.mkdirSync(path.dirname(targetChild), { recursive: true });
    fs.copyFileSync(sourceChild, targetChild);
  }
}

export function preparePublishDirectory(
  extensionRoot: string,
  options: Pick<PackageVsixOptions, "bundleBinaryPath" | "target"> = {},
): string {
  const publishRoot = fs.mkdtempSync(path.join(os.tmpdir(), "tomcat-vsix-stage-"));
  for (const asset of ROOT_ASSETS) {
    fs.copyFileSync(
      path.join(extensionRoot, asset),
      path.join(publishRoot, asset),
    );
  }
  for (const asset of DIRECTORY_ASSETS) {
    copyDirectory(path.join(extensionRoot, asset), path.join(publishRoot, asset));
  }

  const stagedOutRoot = path.join(publishRoot, "out");
  fs.mkdirSync(stagedOutRoot, { recursive: true });
  copyFilteredOut(path.join(extensionRoot, "out"), stagedOutRoot);
  if (options.bundleBinaryPath) {
    const bundledRelativePath = bundledExecutableRelativePath(options.target);
    const bundledTargetPath = path.join(publishRoot, bundledRelativePath);
    fs.mkdirSync(path.dirname(bundledTargetPath), { recursive: true });
    fs.copyFileSync(options.bundleBinaryPath, bundledTargetPath);
    if (!options.target?.startsWith("win")) {
      fs.chmodSync(bundledTargetPath, 0o755);
    }
  }
  return publishRoot;
}

export function listPublishableFiles(
  packageRoot: string,
  extensionRoot = packageRoot,
): string[] {
  const stdout = runVsce(
    ["ls", "--no-dependencies"],
    packageRoot,
    extensionRoot,
    true,
  );
  return stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export function assertPublishableFiles(
  fileList: readonly string[],
  options: Pick<PackageVsixOptions, "bundleBinaryPath" | "target"> = {},
): void {
  const requiredFiles = [...REQUIRED_FILES];
  if (options.bundleBinaryPath) {
    requiredFiles.push(bundledExecutableRelativePath(options.target));
  }

  for (const requiredFile of requiredFiles) {
    if (!hasPath(fileList, requiredFile)) {
      throw new Error(`VSIX source is missing required file: ${requiredFile}`);
    }
  }

  for (const disallowedPrefix of DISALLOWED_PREFIXES) {
    if (hasPrefix(fileList, disallowedPrefix)) {
      throw new Error(`VSIX source should not include files from ${disallowedPrefix}`);
    }
  }

  for (const file of fileList) {
    if (/^out\/.+\/tests\//.test(file)) {
      throw new Error(`VSIX source should not include compiled test output: ${file}`);
    }
  }
}

export function packageVsix(options: PackageVsixOptions = {}): string {
  const extensionRoot = options.extensionRoot ?? path.resolve(__dirname, "..");
  const manifest = readManifest(extensionRoot);
  const defaultOutPath = buildVsixOutPath(extensionRoot, manifest, options.target);
  const outPath = options.outPath ?? defaultOutPath;

  if (!options.skipBuild) {
    run("npm", ["run", "build"], extensionRoot);
  }
  const publishRoot = preparePublishDirectory(extensionRoot, options);
  try {
    const fileList = listPublishableFiles(publishRoot, extensionRoot);
    assertPublishableFiles(fileList, options);

    runVsce(
      buildVscePackageArgs(outPath, options.target),
      publishRoot,
      extensionRoot,
    );
    return outPath;
  } finally {
    fs.rmSync(publishRoot, { force: true, recursive: true });
  }
}

function parseOutPath(argv: readonly string[]): string | undefined {
  const index = argv.indexOf("--out");
  if (index === -1) {
    return undefined;
  }

  const value = argv[index + 1];
  if (!value) {
    throw new Error("--out requires a file path");
  }
  return path.resolve(value);
}

function parseTarget(argv: readonly string[]): string | undefined {
  const index = argv.indexOf("--target");
  if (index === -1) {
    return undefined;
  }

  const value = argv[index + 1];
  if (!value) {
    throw new Error("--target requires a VS Code target");
  }
  return value;
}

function parseBundleBinaryPath(argv: readonly string[]): string | undefined {
  const index = argv.indexOf("--bundle-binary");
  if (index === -1) {
    return undefined;
  }

  const value = argv[index + 1];
  if (!value) {
    throw new Error("--bundle-binary requires a file path");
  }
  return path.resolve(value);
}

function parseSkipBuild(argv: readonly string[]): boolean {
  return argv.includes("--skip-build");
}

function main(): void {
  const argv = process.argv.slice(2);
  const outPath = parseOutPath(argv);
  const target = parseTarget(argv);
  const bundleBinaryPath = parseBundleBinaryPath(argv);
  const skipBuild = parseSkipBuild(argv);
  const result = packageVsix({ bundleBinaryPath, outPath, skipBuild, target });
  console.log(`Packaged VSIX at ${result}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
  }
}
