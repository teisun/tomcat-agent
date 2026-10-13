import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";

import { describe, expect, it } from "vitest";

const guardsModulePath = path.resolve(
  __dirname,
  "..",
  "..",
  ".github",
  "scripts",
  "release",
  "guards.mjs",
);
const guardsPromise = import(pathToFileURL(guardsModulePath).href);

function json(value: unknown): string {
  return `${JSON.stringify(value, null, 2)}\n`;
}

async function createVersionFixture(): Promise<string> {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-release-guards-"));
  await fs.mkdir(path.join(root, "tomcat"));
  await fs.mkdir(path.join(root, "tomcat-vscode-ext"));
  await fs.writeFile(
    path.join(root, "release-versions.json"),
    json({
      cli: "0.1.8",
      extension: { version: "0.1.3", bundledCli: "0.1.8" },
    }),
  );
  await fs.writeFile(
    path.join(root, "tomcat/Cargo.toml"),
    "[package]\nname = \"tomcat\"\nversion = \"0.1.8\"\n",
  );
  await fs.writeFile(
    path.join(root, "tomcat/Cargo.lock"),
    "version = 4\n\n[[package]]\nname = \"tomcat\"\nversion = \"0.1.8\"\n",
  );
  await fs.writeFile(
    path.join(root, "tomcat-vscode-ext/package.json"),
    json({
      name: "tomcat-vscode-ext",
      tomcat: { bundledCliVersion: "0.1.8" },
      version: "0.1.3",
    }),
  );
  await fs.writeFile(
    path.join(root, "tomcat-vscode-ext/package-lock.json"),
    json({
      name: "tomcat-vscode-ext",
      version: "0.1.3",
      lockfileVersion: 3,
      packages: { "": { name: "tomcat-vscode-ext", version: "0.1.3" } },
    }),
  );
  return root;
}

async function mutateJson(
  root: string,
  relativePath: string,
  mutate: (value: Record<string, any>) => void,
): Promise<void> {
  const filePath = path.join(root, relativePath);
  const value = JSON.parse(await fs.readFile(filePath, "utf8"));
  mutate(value);
  await fs.writeFile(filePath, json(value));
}

async function withFixture(callback: (root: string) => Promise<void>): Promise<void> {
  const root = await createVersionFixture();
  try {
    await callback(root);
  } finally {
    await fs.rm(root, { force: true, recursive: true });
  }
}

describe("release guard scripts", () => {
  it("requires all three native jobs before release and never activates the legacy PAT publisher", async () => {
    const workflow = await fs.readFile(path.resolve(__dirname, "../../.github/workflows/release-ext.yml"), "utf8");
    expect(workflow).toContain("needs: [build, verify]");
    expect(workflow).toContain("fail-fast: false");
    for (const [runner, target] of [["macos-14", "darwin-arm64"], ["macos-15-intel", "darwin-x64"], ["ubuntu-22.04", "linux-x64"]]) {
      expect(workflow).toMatch(new RegExp(`runner: ${runner}\\s+target: ${target}`));
    }
    expect(workflow).toContain('npm run verify:vsix -- --release-bundle "${vsix}"');
    expect(workflow).not.toMatch(/secrets\.VSCE_PAT|vsce publish|HAS_VSCE_PAT/);
    expect(workflow.indexOf("sha256sum -c -")).toBeLessThan(workflow.indexOf('tar -xzf "${bundle_dir}/${archive}"'));
  });

  it("verifies the actual release download before extraction and rejects corruption or missing checksum", async () => {
    const workflow = await fs.readFile(path.resolve(__dirname, "../../.github/workflows/release-ext.yml"), "utf8");
    const start = workflow.indexOf('            if [ "${USE_CLI_ARTIFACTS}" != "1" ]; then');
    const endLine = '            tar -xzf "${bundle_dir}/${archive}" -C "${bundle_dir}"';
    const end = workflow.indexOf(endLine, start);
    expect(start).toBeGreaterThan(-1); expect(end).toBeGreaterThan(start);
    const script = workflow.slice(start, end + endLine.length);
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-release-checksum-"));
    const run = promisify(execFile);
    const archive = "tomcat-cli-test.tar.gz";
    const bundleDir = "bundles/native";
    const sums = path.join(root, "bundles/cli-checksums/SHA256SUMS");
    const binary = path.join(root, bundleDir, "tomcat");
    try {
      await fs.mkdir(path.join(root, bundleDir), { recursive: true });
      await fs.mkdir(path.dirname(sums), { recursive: true });
      await fs.mkdir(path.join(root, "input"));
      await fs.writeFile(path.join(root, "input/tomcat"), "fixture executable bytes");
      await run("tar", ["-czf", path.join(root, bundleDir, archive), "-C", path.join(root, "input"), "tomcat"]);
      const checksum = createHash("sha256").update(await fs.readFile(path.join(root, bundleDir, archive))).digest("hex");
      await fs.writeFile(sums, `${checksum}  ${archive}\n`);
      const options = { cwd: root, timeout: 10_000, env: { ...process.env, USE_CLI_ARTIFACTS: "0", archive, bundle_dir: bundleDir } };
      // GitHub build is Linux; shasum provides identical -c semantics on macOS.
      const command = process.platform === "darwin" ? script.replace("sha256sum -c -", "shasum -a 256 -c -") : script;
      await run("bash", ["-euc", command], options);
      expect(await fs.readFile(binary, "utf8")).toBe("fixture executable bytes");
      await fs.rm(binary);
      await fs.appendFile(path.join(root, bundleDir, archive), "corrupted transport bytes");
      await expect(run("bash", ["-euc", command], options)).rejects.toMatchObject({ code: 1 });
      await expect(fs.stat(binary)).rejects.toMatchObject({ code: "ENOENT" });
      await fs.writeFile(sums, `${checksum}  unrelated.tar.gz\n`);
      await expect(run("bash", ["-euc", command], options)).rejects.toMatchObject({ code: 1 });
      await expect(fs.stat(binary)).rejects.toMatchObject({ code: "ENOENT" });
    } finally { await fs.rm(root, { force: true, recursive: true }); }
  });

  it("validates CLI release tags against the authoritative CLI version", async () => {
    const guards = await guardsPromise;
    expect(() => guards.validateCliReleaseTag("cli-v0.1.8", "0.1.8")).not.toThrow();
    expect(() => guards.validateCliReleaseTag("cli-v0.1.7", "0.1.8")).toThrow(
      /CLI release tag mismatch/,
    );
  });

  it("checks every mirror before validating the extension tag", async () => {
    const guards = await guardsPromise;
    await withFixture(async (root) => {
      const versions = guards.readExtensionVersions(root);
      expect(versions).toEqual({
        bundledCliVersion: "0.1.8",
        cliVersion: "0.1.8",
        extensionLockTopLevelVersion: "0.1.3",
        extensionLockVersion: "0.1.3",
        extensionVersion: "0.1.3",
      });
      expect(() => guards.validateExtensionReleaseTag("ext-v0.1.3", versions)).not.toThrow();
      expect(() => guards.validateExtensionReleaseTag("ext-v0.1.4", versions)).toThrow(
        /Extension release tag mismatch/,
      );
    });
  });

  const driftCases: Array<{
    name: string;
    expected: RegExp;
    mutate: (root: string) => Promise<void>;
  }> = [
    {
      name: "root CLI version",
      expected: /Cargo\.toml.*expected 0\.1\.9, got 0\.1\.8/s,
      mutate: (root) => mutateJson(root, "release-versions.json", (value) => {
        value.cli = "0.1.9";
      }),
    },
    {
      name: "Cargo manifest",
      expected: /Cargo\.toml.*expected 0\.1\.8, got 0\.1\.7/,
      mutate: async (root) => {
        const filePath = path.join(root, "tomcat/Cargo.toml");
        await fs.writeFile(filePath, (await fs.readFile(filePath, "utf8")).replace("0.1.8", "0.1.7"));
      },
    },
    {
      name: "Cargo lock",
      expected: /Cargo\.lock.*expected 0\.1\.8, got 0\.1\.7/,
      mutate: async (root) => {
        const filePath = path.join(root, "tomcat/Cargo.lock");
        await fs.writeFile(filePath, (await fs.readFile(filePath, "utf8")).replace("0.1.8", "0.1.7"));
      },
    },
    {
      name: "extension manifest version",
      expected: /package\.json version: expected 0\.1\.3, got 0\.1\.2/,
      mutate: (root) => mutateJson(root, "tomcat-vscode-ext/package.json", (value) => {
        value.version = "0.1.2";
      }),
    },
    {
      name: "bundled CLI pin",
      expected: /tomcat\.bundledCliVersion: expected 0\.1\.8, got 0\.1\.7/,
      mutate: (root) => mutateJson(root, "tomcat-vscode-ext/package.json", (value) => {
        value.tomcat.bundledCliVersion = "0.1.7";
      }),
    },
    {
      name: "extension lock top-level version",
      expected: /package-lock\.json version: expected 0\.1\.3, got 0\.1\.2/,
      mutate: (root) => mutateJson(root, "tomcat-vscode-ext/package-lock.json", (value) => {
        value.version = "0.1.2";
      }),
    },
    {
      name: "extension lock root package version",
      expected: /packages\[""\]\.version: expected 0\.1\.3, got 0\.1\.2/,
      mutate: (root) => mutateJson(root, "tomcat-vscode-ext/package-lock.json", (value) => {
        value.packages[""].version = "0.1.2";
      }),
    },
  ];

  for (const driftCase of driftCases) {
    it(`rejects ${driftCase.name} drift`, async () => {
      const guards = await guardsPromise;
      await withFixture(async (root) => {
        await driftCase.mutate(root);
        expect(() => guards.readRepositoryVersions(root)).toThrow(driftCase.expected);
      });
    });
  }

  it("validates that the pinned CLI release exposes every bundled asset", async () => {
    const guards = await guardsPromise;
    const assetNames = [
      "tomcat-cli-v0.1.8-aarch64-apple-darwin.tar.gz",
      "tomcat-cli-v0.1.8-x86_64-apple-darwin.tar.gz",
      "tomcat-cli-v0.1.8-x86_64-unknown-linux-gnu.tar.gz",
    ];

    expect(() => guards.validateBundledCliAssets("0.1.8", assetNames)).not.toThrow();
    expect(() =>
      guards.validateBundledCliAssets("0.1.8", assetNames.slice(0, 2)),
    ).toThrow(/Missing pinned CLI asset/);
  });
});
