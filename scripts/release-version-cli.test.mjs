import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  applyRepositoryVersionPlan,
  assertRepositoryVersions,
  loadRepositoryVersionSnapshot,
  planRepositoryVersionUpdateFromSnapshot,
} from "./release-version-core.mjs";

const REPO_ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const VERSION_FILES = [
  "release-versions.json",
  "tomcat/Cargo.toml",
  "tomcat/Cargo.lock",
  "tomcat-vscode-ext/package.json",
  "tomcat-vscode-ext/package-lock.json",
];

function json(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function createFixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tomcat-release-version-test-"));
  fs.mkdirSync(path.join(root, "scripts"));
  fs.mkdirSync(path.join(root, "tomcat"));
  fs.mkdirSync(path.join(root, "tomcat-vscode-ext"));
  fs.copyFileSync(
    path.join(REPO_ROOT, "scripts/release-version.mjs"),
    path.join(root, "scripts/release-version.mjs"),
  );
  fs.copyFileSync(
    path.join(REPO_ROOT, "scripts/release-version-core.mjs"),
    path.join(root, "scripts/release-version-core.mjs"),
  );
  fs.writeFileSync(
    path.join(root, "release-versions.json"),
    json({
      cli: "1.2.3",
      extension: { version: "2.3.4", bundledCli: "1.2.2" },
    }),
  );
  fs.writeFileSync(
    path.join(root, "tomcat/Cargo.toml"),
    [
      "[package]",
      "name = \"tomcat\"",
      "version = \"1.2.3\"",
      "edition = \"2021\"",
      "",
      "[dependencies]",
      "example = \"9.9.9\"",
      "",
    ].join("\n"),
  );
  fs.writeFileSync(
    path.join(root, "tomcat/Cargo.lock"),
    [
      "version = 4",
      "",
      "[[package]]",
      "name = \"example\"",
      "version = \"9.9.9\"",
      "",
      "[[package]]",
      "name = \"tomcat\"",
      "version = \"1.2.3\"",
      "",
    ].join("\n"),
  );
  fs.writeFileSync(
    path.join(root, "tomcat-vscode-ext/package.json"),
    json({
      name: "tomcat-vscode-ext",
      version: "2.3.4",
      private: true,
      tomcat: { bundledCliVersion: "1.2.2" },
    }),
  );
  fs.writeFileSync(
    path.join(root, "tomcat-vscode-ext/package-lock.json"),
    json({
      name: "tomcat-vscode-ext",
      version: "2.3.4",
      lockfileVersion: 3,
      packages: {
        "": { name: "tomcat-vscode-ext", version: "2.3.4" },
        "node_modules/example": { version: "9.9.9" },
      },
    }),
  );
  return root;
}

function cleanupFixture(root) {
  const tempRoot = `${fs.realpathSync(os.tmpdir())}${path.sep}`;
  const realRoot = fs.realpathSync(root);
  if (!realRoot.startsWith(tempRoot) || !path.basename(realRoot).startsWith("tomcat-release-version-test-")) {
    throw new Error(`Refusing to clean unexpected fixture path: ${realRoot}`);
  }
  fs.rmSync(realRoot, { force: true, recursive: true });
}

function run(root, ...args) {
  return spawnSync(process.execPath, [path.join(root, "scripts/release-version.mjs"), ...args], {
    cwd: root,
    encoding: "utf8",
  });
}

function readAllFiles(root) {
  return Object.fromEntries(
    VERSION_FILES.map((relativePath) => [
      relativePath,
      fs.readFileSync(path.join(root, relativePath), "utf8"),
    ]),
  );
}

function withFixture(callback) {
  const root = createFixture();
  try {
    callback(root);
  } finally {
    cleanupFixture(root);
  }
}

function expectSuccess(result) {
  assert.equal(result.status, 0, `stderr:\n${result.stderr}\nstdout:\n${result.stdout}`);
}

test("bump --all advances both releases and makes the new extension bundle the new CLI", () => {
  withFixture((root) => {
    const result = run(root, "bump", "--all", "patch");
    expectSuccess(result);
    assert.match(
      result.stdout,
      /CLI: 1\.2\.3 -> 1\.2\.4[\s\S]*extension: 2\.3\.4 -> 2\.3\.5[\s\S]*bundled CLI: 1\.2\.2 -> 1\.2\.4/,
    );
    const snapshot = assertRepositoryVersions(root);
    assert.deepEqual(snapshot.versions, {
      cli: "1.2.4",
      extension: { version: "2.3.5", bundledCli: "1.2.4" },
    });
    assert.match(snapshot.files.cargoManifest.content, /example = "9\.9\.9"/);
    assert.equal(JSON.parse(snapshot.files.extensionLock.content).packages["node_modules/example"].version, "9.9.9");
  });
});

test("CLI-only bump preserves the extension release and bundled pin", () => {
  withFixture((root) => {
    expectSuccess(run(root, "bump", "--cli", "minor"));
    assert.deepEqual(assertRepositoryVersions(root).versions, {
      cli: "1.3.0",
      extension: { version: "2.3.4", bundledCli: "1.2.2" },
    });
  });
});

test("extension-only bump preserves the CLI release and bundled pin", () => {
  withFixture((root) => {
    expectSuccess(run(root, "bump", "--extension", "major"));
    assert.deepEqual(assertRepositoryVersions(root).versions, {
      cli: "1.2.3",
      extension: { version: "3.0.0", bundledCli: "1.2.2" },
    });
  });
});

test("set updates explicit independent values and all mirrors", () => {
  withFixture((root) => {
    expectSuccess(
      run(
        root,
        "set",
        "--cli",
        "1.4.0",
        "--extension",
        "2.4.0",
        "--bundled-cli",
        "1.4.0",
      ),
    );
    assert.deepEqual(assertRepositoryVersions(root).versions, {
      cli: "1.4.0",
      extension: { version: "2.4.0", bundledCli: "1.4.0" },
    });
  });
});

test("sync repairs valid mirror drift and is byte-for-byte idempotent", () => {
  withFixture((root) => {
    const sourcePath = path.join(root, "release-versions.json");
    fs.writeFileSync(
      sourcePath,
      json({
        cli: "1.2.4",
        extension: { version: "2.3.5", bundledCli: "1.2.2" },
      }),
    );

    const first = run(root, "sync");
    expectSuccess(first);
    assert.deepEqual(assertRepositoryVersions(root).versions, {
      cli: "1.2.4",
      extension: { version: "2.3.5", bundledCli: "1.2.2" },
    });
    const afterFirst = readAllFiles(root);
    const second = run(root, "sync");
    expectSuccess(second);
    assert.match(second.stdout, /already synchronized/);
    assert.deepEqual(readAllFiles(root), afterFirst);
  });
});

test("check is read-only and reports a precise mirror drift", () => {
  withFixture((root) => {
    const cargoPath = path.join(root, "tomcat/Cargo.toml");
    fs.writeFileSync(cargoPath, fs.readFileSync(cargoPath, "utf8").replace("1.2.3", "1.2.4"));
    const before = readAllFiles(root);
    const result = run(root, "check");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /tomcat\/Cargo\.toml \[package\]\.version: expected 1\.2\.3, got 1\.2\.4/);
    assert.deepEqual(readAllFiles(root), before);
  });
});

test("changing the bundled pin without a new extension release fails before any write", () => {
  withFixture((root) => {
    const sourcePath = path.join(root, "release-versions.json");
    fs.writeFileSync(
      sourcePath,
      json({
        cli: "1.2.3",
        extension: { version: "2.3.4", bundledCli: "1.2.3" },
      }),
    );
    const before = readAllFiles(root);
    const result = run(root, "sync");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /requires a new extension\.version/);
    assert.deepEqual(readAllFiles(root), before);
  });
});

test("set enforces the bundled pin safety contract", () => {
  withFixture((root) => {
    const before = readAllFiles(root);
    const result = run(root, "set", "--bundled-cli", "1.2.3");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /requires --extension/);
    assert.deepEqual(readAllFiles(root), before);
  });
});

test("malformed input aborts before any target file is written", () => {
  withFixture((root) => {
    const lockPath = path.join(root, "tomcat/Cargo.lock");
    fs.appendFileSync(lockPath, "[[package]]\nname = \"tomcat\"\nversion = \"1.2.3\"\n");
    const before = readAllFiles(root);
    const result = run(root, "sync");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /exactly one \[\[package\]\] named "tomcat"/);
    assert.deepEqual(readAllFiles(root), before);
  });
});

test("a single validated snapshot prevents edits between validation and planning from being overwritten", () => {
  withFixture((root) => {
    const snapshot = assertRepositoryVersions(root);
    const cargoPath = path.join(root, "tomcat/Cargo.toml");
    fs.appendFileSync(cargoPath, "# concurrent edit\n");
    const before = readAllFiles(root);
    const plan = planRepositoryVersionUpdateFromSnapshot(snapshot, {
      cli: "1.2.4",
      extension: { version: "2.3.4", bundledCli: "1.2.2" },
    });
    assert.throws(
      () => applyRepositoryVersionPlan(plan),
      /changed after the version update was planned/,
    );
    assert.deepEqual(readAllFiles(root), before);
  });
});

test("a failed bump does not use sync semantics to hide pre-existing drift", () => {
  withFixture((root) => {
    const lockPath = path.join(root, "tomcat-vscode-ext/package-lock.json");
    const lock = JSON.parse(fs.readFileSync(lockPath, "utf8"));
    lock.packages[""].version = "2.3.3";
    fs.writeFileSync(lockPath, json(lock));
    const before = readAllFiles(root);
    const result = run(root, "bump", "--cli", "patch");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Version mirrors are out of sync/);
    assert.deepEqual(readAllFiles(root), before);
    assert.equal(loadRepositoryVersionSnapshot(root).versions.cli, "1.2.3");
  });
});
