import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export const RELEASE_VERSIONS_FILE = "release-versions.json";
export const VERSION_FIELDS = ["cli", "extension.version", "extension.bundledCli"];

const STABLE_SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const BUMP_KINDS = new Set(["major", "minor", "patch"]);

function fail(message) {
  throw new Error(message);
}

function isPlainObject(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function assertExactKeys(value, expected, label) {
  if (!isPlainObject(value)) {
    fail(`${label} must be an object`);
  }

  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  const missing = wanted.filter((key) => !actual.includes(key));
  const unknown = actual.filter((key) => !wanted.includes(key));
  if (missing.length > 0 || unknown.length > 0) {
    const details = [
      missing.length > 0 ? `missing ${missing.join(", ")}` : null,
      unknown.length > 0 ? `unknown ${unknown.join(", ")}` : null,
    ].filter(Boolean);
    fail(`${label} has invalid fields: ${details.join("; ")}`);
  }
}

export function parseStableVersion(value, label = "version") {
  if (typeof value !== "string") {
    fail(`${label} must be a string in x.y.z form`);
  }
  const match = STABLE_SEMVER.exec(value);
  if (!match) {
    fail(`${label} must be a stable x.y.z version without leading zeroes; got ${JSON.stringify(value)}`);
  }
  return {
    major: BigInt(match[1]),
    minor: BigInt(match[2]),
    patch: BigInt(match[3]),
    value,
  };
}

export function compareStableVersions(left, right) {
  const a = parseStableVersion(left, "left version");
  const b = parseStableVersion(right, "right version");
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] < b[key]) return -1;
    if (a[key] > b[key]) return 1;
  }
  return 0;
}

export function bumpStableVersion(value, kind) {
  const parsed = parseStableVersion(value);
  if (!BUMP_KINDS.has(kind)) {
    fail(`bump kind must be major, minor, or patch; got ${JSON.stringify(kind)}`);
  }

  if (kind === "major") {
    return `${parsed.major + 1n}.0.0`;
  }
  if (kind === "minor") {
    return `${parsed.major}.${parsed.minor + 1n}.0`;
  }
  return `${parsed.major}.${parsed.minor}.${parsed.patch + 1n}`;
}

export function validateReleaseVersions(value, label = RELEASE_VERSIONS_FILE) {
  assertExactKeys(value, ["cli", "extension"], label);
  assertExactKeys(value.extension, ["version", "bundledCli"], `${label}.extension`);

  const normalized = {
    cli: parseStableVersion(value.cli, `${label}.cli`).value,
    extension: {
      version: parseStableVersion(value.extension.version, `${label}.extension.version`).value,
      bundledCli: parseStableVersion(
        value.extension.bundledCli,
        `${label}.extension.bundledCli`,
      ).value,
    },
  };

  if (compareStableVersions(normalized.extension.bundledCli, normalized.cli) > 0) {
    fail(
      `${label}.extension.bundledCli (${normalized.extension.bundledCli}) cannot be newer than `
        + `${label}.cli (${normalized.cli})`,
    );
  }
  return normalized;
}

export function parseReleaseVersions(text, label = RELEASE_VERSIONS_FILE) {
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    fail(`${label} is not valid JSON: ${error.message}`);
  }
  return validateReleaseVersions(value, label);
}

export function serializeReleaseVersions(value) {
  return `${JSON.stringify(validateReleaseVersions(value), null, 2)}\n`;
}

export function readReleaseVersions(repoRoot) {
  const filePath = path.join(repoRoot, RELEASE_VERSIONS_FILE);
  return parseReleaseVersions(fs.readFileSync(filePath, "utf8"), RELEASE_VERSIONS_FILE);
}

function replaceSpan(text, start, end, replacement) {
  return `${text.slice(0, start)}${replacement}${text.slice(end)}`;
}

function findTomlSections(text, headerPattern) {
  const headers = [...text.matchAll(/^[ \t]*(\[\[?[^\r\n]+?\]\]?)[ \t]*(?:#.*)?(?:\r?\n|$)/gm)];
  return headers
    .filter((match) => headerPattern.test(match[1]))
    .map((match) => {
      const headerIndex = headers.indexOf(match);
      return {
        end: headerIndex + 1 < headers.length ? headers[headerIndex + 1].index : text.length,
        start: match.index,
      };
    });
}

function findSingleVersionAssignment(sectionText, label) {
  const matches = [
    ...sectionText.matchAll(
      /^([ \t]*version[ \t]*=[ \t]*")([^"]+)("[ \t]*(?:#.*)?)(?:\r?\n|$)/gm,
    ),
  ];
  if (matches.length !== 1) {
    fail(`${label} must contain exactly one string version assignment; found ${matches.length}`);
  }
  const match = matches[0];
  const versionStart = match.index + match[1].length;
  return {
    end: versionStart + match[2].length,
    start: versionStart,
    version: parseStableVersion(match[2], `${label}.version`).value,
  };
}

function findCargoPackageSection(cargoTomlText, label) {
  const sections = findTomlSections(cargoTomlText, /^\[package\]$/);
  if (sections.length !== 1) {
    fail(`${label} must contain exactly one [package] section; found ${sections.length}`);
  }
  return sections[0];
}

export function readCargoPackageVersion(cargoTomlText, label = "tomcat/Cargo.toml") {
  const section = findCargoPackageSection(cargoTomlText, label);
  return findSingleVersionAssignment(cargoTomlText.slice(section.start, section.end), label).version;
}

export function renderCargoPackageVersion(cargoTomlText, version, label = "tomcat/Cargo.toml") {
  const nextVersion = parseStableVersion(version, "CLI version").value;
  const section = findCargoPackageSection(cargoTomlText, label);
  const assignment = findSingleVersionAssignment(
    cargoTomlText.slice(section.start, section.end),
    label,
  );
  return replaceSpan(
    cargoTomlText,
    section.start + assignment.start,
    section.start + assignment.end,
    nextVersion,
  );
}

function findCargoLockPackage(cargoLockText, packageName, label) {
  const blocks = findTomlSections(cargoLockText, /^\[\[package\]\]$/);
  const matching = [];
  for (const block of blocks) {
    const blockText = cargoLockText.slice(block.start, block.end);
    const names = [
      ...blockText.matchAll(
        /^[ \t]*name[ \t]*=[ \t]*"([^"]+)"[ \t]*(?:#.*)?(?:\r?\n|$)/gm,
      ),
    ];
    if (names.length !== 1) {
      fail(`${label} [[package]] block at byte ${block.start} must contain exactly one name; found ${names.length}`);
    }
    if (names[0][1] === packageName) {
      matching.push(block);
    }
  }
  if (matching.length !== 1) {
    fail(`${label} must contain exactly one [[package]] named ${JSON.stringify(packageName)}; found ${matching.length}`);
  }
  return matching[0];
}

export function readCargoLockPackageVersion(
  cargoLockText,
  packageName = "tomcat",
  label = "tomcat/Cargo.lock",
) {
  const block = findCargoLockPackage(cargoLockText, packageName, label);
  return findSingleVersionAssignment(cargoLockText.slice(block.start, block.end), label).version;
}

export function renderCargoLockPackageVersion(
  cargoLockText,
  version,
  packageName = "tomcat",
  label = "tomcat/Cargo.lock",
) {
  const nextVersion = parseStableVersion(version, "CLI version").value;
  const block = findCargoLockPackage(cargoLockText, packageName, label);
  const assignment = findSingleVersionAssignment(cargoLockText.slice(block.start, block.end), label);
  return replaceSpan(
    cargoLockText,
    block.start + assignment.start,
    block.start + assignment.end,
    nextVersion,
  );
}

function parseJsonObject(text, label) {
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    fail(`${label} is not valid JSON: ${error.message}`);
  }
  if (!isPlainObject(value)) {
    fail(`${label} must contain a JSON object`);
  }
  return value;
}

function requireObject(value, property, label) {
  const nested = value[property];
  if (!isPlainObject(nested)) {
    fail(`${label}.${property} must be an object`);
  }
  return nested;
}

function serializeJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

export function readExtensionManifestVersions(
  manifestText,
  label = "tomcat-vscode-ext/package.json",
) {
  const manifest = parseJsonObject(manifestText, label);
  const tomcat = requireObject(manifest, "tomcat", label);
  return {
    bundledCliVersion: parseStableVersion(
      tomcat.bundledCliVersion,
      `${label}.tomcat.bundledCliVersion`,
    ).value,
    extensionVersion: parseStableVersion(manifest.version, `${label}.version`).value,
  };
}

export function renderExtensionManifestVersions(
  manifestText,
  versions,
  label = "tomcat-vscode-ext/package.json",
) {
  const manifest = parseJsonObject(manifestText, label);
  const tomcat = requireObject(manifest, "tomcat", label);
  // Validate the old document before changing it so malformed mirrors never get silently repaired.
  readExtensionManifestVersions(manifestText, label);
  manifest.version = parseStableVersion(versions.extensionVersion, "extension version").value;
  tomcat.bundledCliVersion = parseStableVersion(
    versions.bundledCliVersion,
    "bundled CLI version",
  ).value;
  return serializeJson(manifest);
}

export function readExtensionLockVersions(
  lockText,
  label = "tomcat-vscode-ext/package-lock.json",
) {
  const lock = parseJsonObject(lockText, label);
  const packages = requireObject(lock, "packages", label);
  const rootPackage = requireObject(packages, "", `${label}.packages`);
  return {
    rootPackageVersion: parseStableVersion(
      rootPackage.version,
      `${label}.packages[\"\"].version`,
    ).value,
    topLevelVersion: parseStableVersion(lock.version, `${label}.version`).value,
  };
}

export function renderExtensionLockVersion(
  lockText,
  version,
  label = "tomcat-vscode-ext/package-lock.json",
) {
  const lock = parseJsonObject(lockText, label);
  const packages = requireObject(lock, "packages", label);
  const rootPackage = requireObject(packages, "", `${label}.packages`);
  // As with the manifest, refuse to hide a malformed old lockfile behind a rewrite.
  readExtensionLockVersions(lockText, label);
  const nextVersion = parseStableVersion(version, "extension version").value;
  lock.version = nextVersion;
  rootPackage.version = nextVersion;
  return serializeJson(lock);
}

export const VERSION_FILE_PATHS = Object.freeze({
  cargoLock: "tomcat/Cargo.lock",
  cargoManifest: "tomcat/Cargo.toml",
  extensionLock: "tomcat-vscode-ext/package-lock.json",
  extensionManifest: "tomcat-vscode-ext/package.json",
  releaseVersions: RELEASE_VERSIONS_FILE,
});

function readVersionFiles(repoRoot) {
  return Object.fromEntries(
    Object.entries(VERSION_FILE_PATHS).map(([key, relativePath]) => [
      key,
      {
        absolutePath: path.join(repoRoot, relativePath),
        content: fs.readFileSync(path.join(repoRoot, relativePath), "utf8"),
        relativePath,
      },
    ]),
  );
}

export function loadRepositoryVersionSnapshot(repoRoot) {
  const files = readVersionFiles(repoRoot);
  return {
    files,
    mirrors: {
      cargoLockVersion: readCargoLockPackageVersion(files.cargoLock.content),
      cargoManifestVersion: readCargoPackageVersion(files.cargoManifest.content),
      extensionLock: readExtensionLockVersions(files.extensionLock.content),
      extensionManifest: readExtensionManifestVersions(files.extensionManifest.content),
    },
    repoRoot,
    versions: parseReleaseVersions(files.releaseVersions.content),
  };
}

export function collectVersionMismatches(snapshot) {
  const expected = snapshot.versions;
  const actual = snapshot.mirrors;
  return [
    {
      actual: actual.cargoManifestVersion,
      expected: expected.cli,
      field: "[package].version",
      file: VERSION_FILE_PATHS.cargoManifest,
    },
    {
      actual: actual.cargoLockVersion,
      expected: expected.cli,
      field: "tomcat package version",
      file: VERSION_FILE_PATHS.cargoLock,
    },
    {
      actual: actual.extensionManifest.extensionVersion,
      expected: expected.extension.version,
      field: "version",
      file: VERSION_FILE_PATHS.extensionManifest,
    },
    {
      actual: actual.extensionManifest.bundledCliVersion,
      expected: expected.extension.bundledCli,
      field: "tomcat.bundledCliVersion",
      file: VERSION_FILE_PATHS.extensionManifest,
    },
    {
      actual: actual.extensionLock.topLevelVersion,
      expected: expected.extension.version,
      field: "version",
      file: VERSION_FILE_PATHS.extensionLock,
    },
    {
      actual: actual.extensionLock.rootPackageVersion,
      expected: expected.extension.version,
      field: "packages[\"\"].version",
      file: VERSION_FILE_PATHS.extensionLock,
    },
  ].filter((entry) => entry.actual !== entry.expected);
}

export function assertRepositoryVersions(repoRoot) {
  const snapshot = loadRepositoryVersionSnapshot(repoRoot);
  const mismatches = collectVersionMismatches(snapshot);
  if (mismatches.length > 0) {
    const details = mismatches.map(
      ({ actual, expected, field, file }) =>
        `  - ${file} ${field}: expected ${expected}, got ${actual}`,
    );
    fail(
      [
        "Version mirrors are out of sync with release-versions.json:",
        ...details,
        "Run: node scripts/release-version.mjs sync",
      ].join("\n"),
    );
  }
  return snapshot;
}

export function planRepositoryVersionUpdateFromSnapshot(snapshot, nextVersions) {
  const versions = validateReleaseVersions(nextVersions ?? snapshot.versions);
  const currentExtension = snapshot.mirrors.extensionManifest;
  if (
    versions.extension.bundledCli !== currentExtension.bundledCliVersion
    && versions.extension.version === currentExtension.extensionVersion
  ) {
    fail(
      "Changing extension.bundledCli changes the VSIX contents and therefore requires a new "
        + "extension.version in the same operation",
    );
  }

  const nextContent = {
    cargoLock: renderCargoLockPackageVersion(snapshot.files.cargoLock.content, versions.cli),
    cargoManifest: renderCargoPackageVersion(snapshot.files.cargoManifest.content, versions.cli),
    extensionLock: renderExtensionLockVersion(
      snapshot.files.extensionLock.content,
      versions.extension.version,
    ),
    extensionManifest: renderExtensionManifestVersions(
      snapshot.files.extensionManifest.content,
      {
        bundledCliVersion: versions.extension.bundledCli,
        extensionVersion: versions.extension.version,
      },
    ),
    releaseVersions: serializeReleaseVersions(versions),
  };

  const changes = Object.keys(VERSION_FILE_PATHS)
    .map((key) => ({
      absolutePath: snapshot.files[key].absolutePath,
      nextContent: nextContent[key],
      previousContent: snapshot.files[key].content,
      relativePath: snapshot.files[key].relativePath,
    }))
    .filter((change) => change.nextContent !== change.previousContent);

  return {
    changes,
    previousVersions: snapshot.versions,
    repoRoot: snapshot.repoRoot,
    versions,
  };
}

export function planRepositoryVersionUpdate(repoRoot, nextVersions) {
  return planRepositoryVersionUpdateFromSnapshot(
    loadRepositoryVersionSnapshot(repoRoot),
    nextVersions,
  );
}

export function applyRepositoryVersionPlan(plan) {
  const staged = [];
  try {
    for (const change of plan.changes) {
      const tempPath = path.join(
        path.dirname(change.absolutePath),
        `.${path.basename(change.absolutePath)}.tomcat-version-${process.pid}-${randomUUID()}`,
      );
      const mode = fs.statSync(change.absolutePath).mode & 0o777;
      fs.writeFileSync(tempPath, change.nextContent, { encoding: "utf8", flag: "wx", mode });
      staged.push({ change, tempPath });
    }
    for (const { change } of staged) {
      const currentContent = fs.readFileSync(change.absolutePath, "utf8");
      if (currentContent !== change.previousContent) {
        fail(
          `${change.relativePath} changed after the version update was planned; `
            + "no version files were replaced",
        );
      }
    }
    for (const { change, tempPath } of staged) {
      fs.renameSync(tempPath, change.absolutePath);
    }
  } finally {
    for (const { tempPath } of staged) {
      if (fs.existsSync(tempPath)) {
        fs.unlinkSync(tempPath);
      }
    }
  }
  return plan.changes;
}
