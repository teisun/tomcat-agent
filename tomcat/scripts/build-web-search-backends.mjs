#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const crateRoot = resolve(scriptDir, "..");
const pluginDir = resolve(crateRoot, "assets/plugins/web-search-backends");

const result = spawnSync(
  "cargo",
  ["run", "--bin", "tomcat", "--", "plugin", "build", pluginDir],
  {
    cwd: crateRoot,
    stdio: "inherit"
  }
);

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
