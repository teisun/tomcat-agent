import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { pathToFileURL } from "node:url";

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
