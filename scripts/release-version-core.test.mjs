import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  assertRepositoryVersions,
  bumpStableVersion,
  compareStableVersions,
  parseReleaseVersions,
  parseStableVersion,
  readCargoLockPackageVersion,
  readCargoPackageVersion,
  readExtensionLockVersions,
  readExtensionManifestVersions,
  renderCargoLockPackageVersion,
  renderCargoPackageVersion,
  renderExtensionLockVersion,
  renderExtensionManifestVersions,
  serializeReleaseVersions,
  validateReleaseVersions,
} from "./release-version-core.mjs";

const REPO_ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const VALID_VERSIONS = {
  cli: "1.2.3",
  extension: { version: "4.5.6", bundledCli: "1.2.2" },
};

function json(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

test("release version schema accepts only the three stable release facts", () => {
  assert.deepEqual(validateReleaseVersions(VALID_VERSIONS), VALID_VERSIONS);
  assert.deepEqual(parseReleaseVersions(json(VALID_VERSIONS)), VALID_VERSIONS);
  assert.equal(serializeReleaseVersions(VALID_VERSIONS), json(VALID_VERSIONS));

  assert.throws(
    () => validateReleaseVersions({ ...VALID_VERSIONS, extra: "1.0.0" }),
    /unknown extra/,
  );
  assert.throws(
    () => validateReleaseVersions({ cli: "1.2.3", extension: { version: "4.5.6" } }),
    /missing bundledCli/,
  );
  assert.throws(
    () => validateReleaseVersions({ ...VALID_VERSIONS, cli: "01.2.3" }),
    /without leading zeroes/,
  );
  assert.throws(
    () => validateReleaseVersions({ ...VALID_VERSIONS, cli: "1.2.3-beta.1" }),
    /stable x\.y\.z/,
  );
  assert.throws(
    () => validateReleaseVersions({
      cli: "1.2.3",
      extension: { version: "4.5.6", bundledCli: "1.2.4" },
    }),
    /cannot be newer/,
  );
});

test("stable version operations are exact and do not lose large integers", () => {
  assert.deepEqual(parseStableVersion("0.0.0"), {
    major: 0n,
    minor: 0n,
    patch: 0n,
    value: "0.0.0",
  });
  assert.equal(bumpStableVersion("1.2.3", "major"), "2.0.0");
  assert.equal(bumpStableVersion("1.2.3", "minor"), "1.3.0");
  assert.equal(bumpStableVersion("1.2.3", "patch"), "1.2.4");
  assert.equal(
    bumpStableVersion("999999999999999999999999.2.3", "major"),
    "1000000000000000000000000.0.0",
  );
  assert.equal(compareStableVersions("10.0.0", "9.999.999"), 1);
  assert.equal(compareStableVersions("1.2.3", "1.2.3"), 0);
  assert.equal(compareStableVersions("1.2.2", "1.2.3"), -1);
  assert.throws(() => bumpStableVersion("1.2.3", "banana"), /major, minor, or patch/);
});

test("Cargo.toml renderer changes only [package].version", () => {
  const source = [
    "# version = \"9.9.9\"",
    "[package]",
    "name = \"tomcat\"",
    "version = \"1.2.3\" # release mirror",
    "edition = \"2021\"",
    "",
    "[dependencies]",
    "example = { version = \"7.8.9\" }",
    "",
  ].join("\n");
  const expected = source.replace(
    "version = \"1.2.3\" # release mirror",
    "version = \"2.0.0\" # release mirror",
  );

  assert.equal(readCargoPackageVersion(source), "1.2.3");
  assert.equal(renderCargoPackageVersion(source, "2.0.0"), expected);
  assert.equal(readCargoPackageVersion(expected), "2.0.0");
  assert.throws(
    () => readCargoPackageVersion(`${source}[package]\nversion = \"1.2.3\"\n`),
    /exactly one \[package\]/,
  );
  assert.throws(
    () => readCargoPackageVersion("[package]\nversion = \"1.2.3\" trailing\n"),
    /exactly one string version assignment/,
  );
});

test("Cargo.lock renderer targets the unique tomcat package block", () => {
  const source = [
    "version = 4",
    "",
    "[[package]]",
    "name = \"alpha\"",
    "version = \"9.9.9\"",
    "",
    "[[package]]",
    "name = \"tomcat\"",
    "version = \"1.2.3\"",
    "dependencies = []",
    "",
  ].join("\n");
  const expected = source.replace(
    "name = \"tomcat\"\nversion = \"1.2.3\"",
    "name = \"tomcat\"\nversion = \"2.0.0\"",
  );

  assert.equal(readCargoLockPackageVersion(source), "1.2.3");
  assert.equal(renderCargoLockPackageVersion(source, "2.0.0"), expected);
  assert.match(expected, /name = "alpha"\nversion = "9\.9\.9"/);
  assert.throws(
    () => readCargoLockPackageVersion(`${source}[[package]]\nname = \"tomcat\"\nversion = \"1.2.3\"\n`),
    /exactly one \[\[package\]\] named "tomcat"/,
  );
  assert.throws(
    () => readCargoLockPackageVersion("[[package]]\nversion = \"1.2.3\"\n"),
    /exactly one name/,
  );
});

test("extension manifest renderer preserves unrelated fields", () => {
  const source = json({
    name: "tomcat-vscode-ext",
    version: "1.2.3",
    tomcat: { bundledCliVersion: "1.0.0", futureField: true },
    scripts: { build: "echo unchanged" },
  });
  const rendered = renderExtensionManifestVersions(source, {
    extensionVersion: "2.0.0",
    bundledCliVersion: "1.1.0",
  });
  const parsed = JSON.parse(rendered);

  assert.deepEqual(readExtensionManifestVersions(rendered), {
    extensionVersion: "2.0.0",
    bundledCliVersion: "1.1.0",
  });
  assert.deepEqual(parsed.scripts, { build: "echo unchanged" });
  assert.equal(parsed.tomcat.futureField, true);
  assert.equal(
    renderExtensionManifestVersions(rendered, {
      extensionVersion: "2.0.0",
      bundledCliVersion: "1.1.0",
    }),
    rendered,
  );
  assert.throws(
    () => readExtensionManifestVersions(json({ version: "1.2.3" })),
    /tomcat must be an object/,
  );
});

test("extension lock renderer keeps top-level and root package versions together", () => {
  const source = json({
    name: "tomcat-vscode-ext",
    version: "1.2.3",
    lockfileVersion: 3,
    packages: {
      "": { name: "tomcat-vscode-ext", version: "1.2.3", license: "MIT" },
      "node_modules/example": { version: "9.9.9" },
    },
  });
  const rendered = renderExtensionLockVersion(source, "2.0.0");
  const parsed = JSON.parse(rendered);

  assert.deepEqual(readExtensionLockVersions(rendered), {
    rootPackageVersion: "2.0.0",
    topLevelVersion: "2.0.0",
  });
  assert.equal(parsed.packages["node_modules/example"].version, "9.9.9");
  assert.equal(renderExtensionLockVersion(rendered, "2.0.0"), rendered);
  assert.throws(
    () => readExtensionLockVersions(json({ version: "1.2.3", packages: {} })),
    /packages\. must be an object/,
  );
});

test("private GUI manifests deliberately have no release version", () => {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(REPO_ROOT, "tomcat-vscode-ext/gui/package.json"), "utf8"),
  );
  const lock = JSON.parse(
    fs.readFileSync(path.join(REPO_ROOT, "tomcat-vscode-ext/gui/package-lock.json"), "utf8"),
  );
  assert.equal(Object.hasOwn(manifest, "version"), false);
  assert.equal(Object.hasOwn(lock, "version"), false);
  assert.equal(Object.hasOwn(lock.packages[""], "version"), false);
});

test("the checked-in repository mirrors match release-versions.json", () => {
  assert.doesNotThrow(() => assertRepositoryVersions(REPO_ROOT));
});
