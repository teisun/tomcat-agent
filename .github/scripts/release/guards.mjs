import fs from "node:fs";
import path from "node:path";

export const CLI_BUNDLE_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
];

export function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

export function parseCargoVersion(cargoTomlText) {
  const packageSectionMatch = cargoTomlText.match(/\[package\][\s\S]*?(?=\n\[|$)/);
  const packageSection = packageSectionMatch ? packageSectionMatch[0] : cargoTomlText;
  const versionMatch = packageSection.match(/^\s*version\s*=\s*"([^"]+)"/m);
  if (!versionMatch) {
    throw new Error("Could not locate Cargo package version");
  }
  return versionMatch[1];
}

export function expectedCliTag(version) {
  return `cli-v${version}`;
}

export function expectedExtTag(version) {
  return `ext-v${version}`;
}

export function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label} mismatch: expected ${expected}, got ${actual}`);
  }
}

export function readExtensionVersions(repoRoot) {
  const extensionManifest = readJson(path.join(repoRoot, "tomcat-vscode-ext", "package.json"));
  const guiManifest = readJson(path.join(repoRoot, "tomcat-vscode-ext", "gui", "package.json"));
  const extensionLock = readJson(path.join(repoRoot, "tomcat-vscode-ext", "package-lock.json"));
  const guiLock = readJson(path.join(repoRoot, "tomcat-vscode-ext", "gui", "package-lock.json"));
  const bundledCliVersion = extensionManifest.tomcat?.bundledCliVersion;

  if (!bundledCliVersion || typeof bundledCliVersion !== "string") {
    throw new Error("tomcat-vscode-ext/package.json is missing tomcat.bundledCliVersion");
  }

  return {
    bundledCliVersion,
    extensionLockVersion: extensionLock.packages?.[""]?.version,
    extensionVersion: extensionManifest.version,
    guiLockVersion: guiLock.packages?.[""]?.version,
    guiVersion: guiManifest.version,
  };
}

export function validateCliReleaseTag(tag, cargoVersion) {
  assertEqual(tag, expectedCliTag(cargoVersion), "CLI release tag");
}

export function validateExtensionReleaseTag(tag, versions) {
  assertEqual(tag, expectedExtTag(versions.extensionVersion), "Extension release tag");
  assertEqual(
    versions.guiVersion,
    versions.extensionVersion,
    "GUI package version",
  );
  assertEqual(
    versions.extensionLockVersion,
    versions.extensionVersion,
    "Extension package-lock version",
  );
  assertEqual(
    versions.guiLockVersion,
    versions.extensionVersion,
    "GUI package-lock version",
  );
}

export function expectedCliAssetNames(cliVersion) {
  return CLI_BUNDLE_TARGETS.map((target) => `tomcat-cli-v${cliVersion}-${target}.tar.gz`);
}

export function validateBundledCliAssets(cliVersion, assetNames) {
  const available = new Set(assetNames);
  for (const expected of expectedCliAssetNames(cliVersion)) {
    if (!available.has(expected)) {
      throw new Error(`Missing pinned CLI asset: ${expected}`);
    }
  }
}
