#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  applyRepositoryVersionPlan,
  assertRepositoryVersions,
  bumpStableVersion,
  loadRepositoryVersionSnapshot,
  planRepositoryVersionUpdateFromSnapshot,
} from "./release-version-core.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const DEFAULT_REPO_ROOT = path.dirname(path.dirname(SCRIPT_PATH));
const USAGE = `Usage:
  node scripts/release-version.mjs bump --all <major|minor|patch>
  node scripts/release-version.mjs bump --cli <major|minor|patch>
  node scripts/release-version.mjs bump --extension <major|minor|patch>
  node scripts/release-version.mjs set [--cli <x.y.z>] [--extension <x.y.z>] [--bundled-cli <x.y.z>]
  node scripts/release-version.mjs sync
  node scripts/release-version.mjs check`;

function fail(message) {
  throw new Error(`${message}\n\n${USAGE}`);
}

function parseOptionPairs(args, allowed) {
  if (args.length === 0 || args.length % 2 !== 0) {
    fail("Options must be provided as --name value pairs");
  }
  const options = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const option = args[index];
    const value = args[index + 1];
    if (!allowed.has(option)) {
      fail(`Unknown option ${JSON.stringify(option)}`);
    }
    if (options.has(option)) {
      fail(`Option ${option} may only be provided once`);
    }
    options.set(option, value);
  }
  return options;
}

function cloneVersions(versions) {
  return {
    cli: versions.cli,
    extension: {
      bundledCli: versions.extension.bundledCli,
      version: versions.extension.version,
    },
  };
}

function formatVersions(versions) {
  return `CLI ${versions.cli}; extension ${versions.extension.version}; bundled CLI ${versions.extension.bundledCli}`;
}

function printAppliedPlan(plan, writeLine) {
  if (plan.changes.length === 0) {
    writeLine(`Versions already synchronized: ${formatVersions(plan.versions)}`);
    return;
  }

  const releaseChanges = [
    ["CLI", plan.previousVersions.cli, plan.versions.cli],
    ["extension", plan.previousVersions.extension.version, plan.versions.extension.version],
    [
      "bundled CLI",
      plan.previousVersions.extension.bundledCli,
      plan.versions.extension.bundledCli,
    ],
  ].filter(([, previous, next]) => previous !== next);

  if (releaseChanges.length > 0) {
    writeLine("Updated release values:");
    for (const [label, previous, next] of releaseChanges) {
      writeLine(`  - ${label}: ${previous} -> ${next}`);
    }
  } else {
    writeLine("Release values unchanged; synchronized generated mirrors.");
  }
  writeLine("Updated files:");
  for (const change of plan.changes) {
    writeLine(`  - ${change.relativePath}`);
  }
}

function requireNoArguments(command, args) {
  if (args.length > 0) {
    fail(`${command} does not accept arguments`);
  }
}

function planBump(repoRoot, args) {
  const snapshot = assertRepositoryVersions(repoRoot);
  const options = parseOptionPairs(args, new Set(["--all", "--cli", "--extension"]));
  if (options.size !== 1) {
    fail("bump requires exactly one of --all, --cli, or --extension");
  }

  const versions = cloneVersions(snapshot.versions);
  if (options.has("--all")) {
    const kind = options.get("--all");
    versions.cli = bumpStableVersion(versions.cli, kind);
    versions.extension.version = bumpStableVersion(versions.extension.version, kind);
    versions.extension.bundledCli = versions.cli;
  } else if (options.has("--cli")) {
    versions.cli = bumpStableVersion(versions.cli, options.get("--cli"));
  } else {
    versions.extension.version = bumpStableVersion(
      versions.extension.version,
      options.get("--extension"),
    );
  }
  return planRepositoryVersionUpdateFromSnapshot(snapshot, versions);
}

function planSet(repoRoot, args) {
  const snapshot = assertRepositoryVersions(repoRoot);
  const options = parseOptionPairs(
    args,
    new Set(["--cli", "--extension", "--bundled-cli"]),
  );
  const versions = cloneVersions(snapshot.versions);
  if (options.has("--cli")) versions.cli = options.get("--cli");
  if (options.has("--extension")) versions.extension.version = options.get("--extension");
  if (options.has("--bundled-cli")) {
    const bundledCli = options.get("--bundled-cli");
    if (
      bundledCli !== snapshot.versions.extension.bundledCli
      && !options.has("--extension")
    ) {
      fail("Changing --bundled-cli requires --extension in the same set command");
    }
    versions.extension.bundledCli = bundledCli;
  }
  return planRepositoryVersionUpdateFromSnapshot(snapshot, versions);
}

export function runReleaseVersionCommand(
  argv,
  { repoRoot = DEFAULT_REPO_ROOT, writeLine = console.log } = {},
) {
  const [command, ...args] = argv;
  if (!command || command === "--help" || command === "-h" || command === "help") {
    writeLine(USAGE);
    return { command: "help" };
  }

  if (command === "check") {
    requireNoArguments(command, args);
    const snapshot = assertRepositoryVersions(repoRoot);
    writeLine(`Version check passed: ${formatVersions(snapshot.versions)}`);
    return { command, snapshot };
  }

  let plan;
  if (command === "sync") {
    requireNoArguments(command, args);
    const snapshot = loadRepositoryVersionSnapshot(repoRoot);
    plan = planRepositoryVersionUpdateFromSnapshot(snapshot, snapshot.versions);
  } else if (command === "bump") {
    plan = planBump(repoRoot, args);
  } else if (command === "set") {
    plan = planSet(repoRoot, args);
  } else {
    fail(`Unknown command ${JSON.stringify(command)}`);
  }

  applyRepositoryVersionPlan(plan);
  printAppliedPlan(plan, writeLine);
  return { command, plan };
}

const invokedAsScript = process.argv[1]
  && fs.realpathSync(process.argv[1]) === fs.realpathSync(SCRIPT_PATH);
if (invokedAsScript) {
  try {
    runReleaseVersionCommand(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
