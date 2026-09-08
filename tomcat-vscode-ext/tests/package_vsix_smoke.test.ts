import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { beforeAll, describe, expect, it } from "vitest";
import { spawn } from "node:child_process";
import { BUILD_RECORD, recordBuildArtifacts, REQUIRED_OUTPUTS } from "../scripts/buildArtifacts";

import { crc32 } from "node:zlib";

import {
  assertPrebuiltArtifactsFresh,
  assertPublishableFiles,
  assertVsixExtractable,
  buildVscePackageArgs,
  buildVsixOutPath,
  bundledExecutableRelativePath,
  listPublishableFiles,
  packageVsix,
  PREBUILT_VSIX_ENV,
  preparePublishDirectory,
} from "../scripts/package-vsix";
import { extractVsixLikeCursor } from "../scripts/vsix-extractable";

function writeU16(value: number): Buffer {
  const buffer = Buffer.alloc(2);
  buffer.writeUInt16LE(value);
  return buffer;
}

function writeU32(value: number): Buffer {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32LE(value);
  return buffer;
}

function makeStoredZip(fileName: string, content: Buffer): Buffer {
  const name = Buffer.from(fileName);
  const crc = crc32(content) >>> 0;
  const local = Buffer.concat([
    Buffer.from([0x50, 0x4b, 0x03, 0x04]),
    writeU16(20),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU32(crc),
    writeU32(content.length),
    writeU32(content.length),
    writeU16(name.length),
    writeU16(0),
    name,
    content,
  ]);
  const central = Buffer.concat([
    Buffer.from([0x50, 0x4b, 0x01, 0x02]),
    writeU16(0x031e),
    writeU16(20),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU32(crc),
    writeU32(content.length),
    writeU32(content.length),
    writeU16(name.length),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU16(0),
    writeU32(0),
    writeU32(0),
    name,
  ]);
  const eocd = Buffer.concat([
    Buffer.from([0x50, 0x4b, 0x05, 0x06]),
    writeU16(0),
    writeU16(0),
    writeU16(1),
    writeU16(1),
    writeU32(central.length),
    writeU32(local.length),
    writeU16(0),
  ]);
  return Buffer.concat([local, central, eocd]);
}

describe("VSIX packaging", () => {
  beforeAll(async () => {
    const root = path.resolve(__dirname, "..");
    if (!process.env[PREBUILT_VSIX_ENV]) {
      await new Promise<void>((resolve, reject) => {
        const child = spawn("npm", ["run", "build"], { cwd: root, stdio: "inherit", detached: process.platform !== "win32" });
        let timedOut = false;
        const timer = setTimeout(() => {
          timedOut = true;
          if (process.platform !== "win32" && child.pid) {
            try { process.kill(-child.pid, "SIGKILL"); } catch { /* already exited */ }
          }
          child.kill("SIGKILL");
        }, 180_000);
        child.once("error", (error) => { clearTimeout(timer); reject(error); });
        child.once("close", (code) => {
          clearTimeout(timer);
          if (code === 0 && !timedOut) resolve();
          else reject(new Error(`package test build failed: code=${code}, timedOut=${timedOut}`));
        });
      });
    }
    assertPrebuiltArtifactsFresh(root);
  }, 200_000);

  it(
    "packages non-interactively and excludes source-only directories",
    async () => {
      const extensionRoot = path.resolve(__dirname, "..");
      const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-test-"));
      const vsixPath = path.join(tempRoot, "tomcat-vscode-ext.vsix");
      let publishRoot: string | undefined;

      try {
        const packaged = packageVsix({ extensionRoot, outPath: vsixPath, skipBuild: true });
        publishRoot = preparePublishDirectory(extensionRoot);
        const fileList = listPublishableFiles(publishRoot, extensionRoot);
        assertPublishableFiles(fileList);
        expect(fileList).toContain("CHANGELOG.md");
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
        expect(() => assertVsixExtractable(packaged)).not.toThrow();
      } finally {
        if (publishRoot) {
          await fs.rm(publishRoot, { force: true, recursive: true });
        }
        await fs.rm(tempRoot, { force: true, recursive: true });
      }
    },
    300_000,
  );

  it("keeps bundling opt-in and stages the bundled executable when requested", async () => {
    const extensionRoot = path.resolve(__dirname, "..");
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-bundle-"));
    const fakeBinaryPath = path.join(tempRoot, "fake-tomcat");
    const vsixPath = path.join(tempRoot, "tomcat-vscode-ext-bundled.vsix");
    let plainPublishRoot: string | undefined;
    let bundledPublishRoot: string | undefined;

    try {
      await fs.writeFile(fakeBinaryPath, "#!/usr/bin/env bash\nprintf 'fake'\n", "utf8");
      await fs.chmod(fakeBinaryPath, 0o755);

      plainPublishRoot = preparePublishDirectory(extensionRoot);
      const plainFileList = listPublishableFiles(plainPublishRoot, extensionRoot);
      expect(plainFileList).not.toContain(bundledExecutableRelativePath("linux-x64"));

      bundledPublishRoot = preparePublishDirectory(extensionRoot, {
        bundleBinaryPath: fakeBinaryPath,
        target: "linux-x64",
      });
      const bundledFileList = listPublishableFiles(bundledPublishRoot, extensionRoot);
      assertPublishableFiles(bundledFileList, {
        bundleBinaryPath: fakeBinaryPath,
        target: "linux-x64",
      });
      expect(bundledFileList).toContain("bin/tomcat");

      const packaged = packageVsix({
        bundleBinaryPath: fakeBinaryPath,
        extensionRoot,
        outPath: vsixPath,
        target: "linux-x64",
        skipBuild: true,
      });
      const stat = await fs.stat(packaged);
      expect(stat.isFile()).toBe(true);
    } finally {
      if (plainPublishRoot) {
        await fs.rm(plainPublishRoot, { force: true, recursive: true });
      }
      if (bundledPublishRoot) {
        await fs.rm(bundledPublishRoot, { force: true, recursive: true });
      }
      await fs.rm(tempRoot, { force: true, recursive: true });
    }
  }, 300_000);

  it("builds target-aware package args and default output paths", () => {
    const extensionRoot = path.resolve(__dirname, "..");

    expect(
      buildVsixOutPath(extensionRoot, { name: "tomcat-vscode-ext", version: "0.1.3" }, "linux-x64"),
    ).toBe(path.join(extensionRoot, "tomcat-vscode-ext-0.1.3-linux-x64.vsix"));
    expect(
      buildVscePackageArgs("/tmp/tomcat-vscode-ext.vsix", "linux-x64"),
    ).toEqual([
      "package",
      "--no-dependencies",
      "--target",
      "linux-x64",
      "--out",
      "/tmp/tomcat-vscode-ext.vsix",
    ]);
  });

  it.each(["missing-record", "bad-record", "source", "config", "missing-output", "corrupt-output"])(
    "rejects skip-build reuse with %s", async (fault) => {
      const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-freshness-"));
      try {
        for (const file of [...REQUIRED_OUTPUTS, "src/extension.ts", "gui/src/App.tsx", "tsconfig.json", "out/nested/helper.js"]) {
          await fs.mkdir(path.dirname(path.join(root, file)), { recursive: true });
          await fs.writeFile(path.join(root, file), "export {};\n");
        }
        recordBuildArtifacts(root);
        expect(() => assertPrebuiltArtifactsFresh(root)).not.toThrow();
        if (fault === "missing-record") await fs.rm(path.join(root, BUILD_RECORD));
        if (fault === "bad-record") await fs.writeFile(path.join(root, BUILD_RECORD), "broken");
        if (fault === "source") await fs.writeFile(path.join(root, "gui/src/App.tsx"), "changed source");
        if (fault === "config") await fs.writeFile(path.join(root, "tsconfig.json"), "changed config");
        if (fault === "missing-output") await fs.rm(path.join(root, "out/nested/helper.js"));
        if (fault === "corrupt-output") {
          const file = path.join(root, "out/extension.js");
          const stat = await fs.stat(file);
          await fs.writeFile(file, "corrupt output");
          await fs.utimes(file, stat.atime, stat.mtime); // timestamps cannot hide corruption
        }
        expect(() => assertPrebuiltArtifactsFresh(root)).toThrow(/build/);
      } finally { await fs.rm(root, { force: true, recursive: true }); }
    },
  );

  it("rejects a VSIX that Cursor's unzipper cannot extract", async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-integrity-"));
    const intactPath = path.join(root, "intact.vsix");
    const mismatchPath = path.join(root, "mismatch.vsix");
    const truncatedPath = path.join(root, "truncated.vsix");

    try {
      const intact = makeStoredZip("hello.txt", Buffer.from("hi\n"));
      await fs.writeFile(intactPath, intact);
      await expect(extractVsixLikeCursor(intactPath)).resolves.toBeUndefined();

      const mismatch = Buffer.from(intact);
      mismatch.writeUInt32LE(0xdeadbeef, 0);
      await fs.writeFile(mismatchPath, mismatch);
      await expect(extractVsixLikeCursor(mismatchPath)).rejects.toThrow(
        /invalid local file header signature/,
      );

      await fs.writeFile(truncatedPath, intact.subarray(0, intact.length - 10));
      await expect(extractVsixLikeCursor(truncatedPath)).rejects.toThrow();
    } finally {
      await fs.rm(root, { force: true, recursive: true });
    }
  });
});
