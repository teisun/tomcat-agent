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

describe("release guard scripts", () => {
  it("validates CLI release tags against Cargo.toml versions", async () => {
    const guards = await guardsPromise;
    expect(() => guards.validateCliReleaseTag("cli-v0.1.8", "0.1.8")).not.toThrow();
    expect(() => guards.validateCliReleaseTag("cli-v0.1.7", "0.1.8")).toThrow(
      /CLI release tag mismatch/,
    );
  });

  it("validates extension tag, gui version, and lockfile versions together", async () => {
    const guards = await guardsPromise;
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-release-guards-"));
    const extensionRoot = path.join(tempRoot, "tomcat-vscode-ext");
    const guiRoot = path.join(extensionRoot, "gui");

    try {
      await fs.mkdir(guiRoot, { recursive: true });
      await fs.writeFile(
        path.join(extensionRoot, "package.json"),
        JSON.stringify({
          name: "tomcat-vscode-ext",
          tomcat: { bundledCliVersion: "0.1.8" },
          version: "0.1.3",
        }),
      );
      await fs.writeFile(
        path.join(guiRoot, "package.json"),
        JSON.stringify({
          name: "tomcat-vscode-ext-gui",
          version: "0.1.3",
        }),
      );
      await fs.writeFile(
        path.join(extensionRoot, "package-lock.json"),
        JSON.stringify({
          packages: {
            "": { version: "0.1.3" },
          },
        }),
      );
      await fs.writeFile(
        path.join(guiRoot, "package-lock.json"),
        JSON.stringify({
          packages: {
            "": { version: "0.1.3" },
          },
        }),
      );

      const versions = guards.readExtensionVersions(tempRoot);
      expect(versions.bundledCliVersion).toBe("0.1.8");
      expect(() => guards.validateExtensionReleaseTag("ext-v0.1.3", versions)).not.toThrow();
      expect(() => guards.validateExtensionReleaseTag("ext-v0.1.4", versions)).toThrow(
        /Extension release tag mismatch/,
      );
    } finally {
      await fs.rm(tempRoot, { force: true, recursive: true });
    }
  });

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
