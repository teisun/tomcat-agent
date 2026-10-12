import {
  assertRepositoryVersions,
  readCargoPackageVersion,
} from "../../../scripts/release-version-core.mjs";

export const CLI_BUNDLE_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
];

export function parseCargoVersion(cargoTomlText) {
  return readCargoPackageVersion(cargoTomlText);
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

export function readRepositoryVersions(repoRoot) {
  const snapshot = assertRepositoryVersions(repoRoot);
  return {
    bundledCliVersion: snapshot.versions.extension.bundledCli,
    cliVersion: snapshot.versions.cli,
    extensionLockTopLevelVersion: snapshot.mirrors.extensionLock.topLevelVersion,
    extensionLockVersion: snapshot.mirrors.extensionLock.rootPackageVersion,
    extensionVersion: snapshot.versions.extension.version,
  };
}

// Kept as the extension guard's public name; it now validates every repository mirror first.
export function readExtensionVersions(repoRoot) {
  return readRepositoryVersions(repoRoot);
}

export function validateCliReleaseTag(tag, cliVersion) {
  assertEqual(tag, expectedCliTag(cliVersion), "CLI release tag");
}

export function validateExtensionReleaseTag(tag, versions) {
  assertEqual(tag, expectedExtTag(versions.extensionVersion), "Extension release tag");
  assertEqual(
    versions.extensionLockTopLevelVersion,
    versions.extensionVersion,
    "Extension package-lock top-level version",
  );
  assertEqual(
    versions.extensionLockVersion,
    versions.extensionVersion,
    "Extension package-lock root package version",
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
