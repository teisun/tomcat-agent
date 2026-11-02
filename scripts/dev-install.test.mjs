import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

for (const exitCode of [0, 37]) {
  test(`Windows dev-install forwards one Cargo invocation and exit ${exitCode}`, { skip: process.platform !== "win32" }, () => {
    const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "tomcat-dev-install-中文 "));
    try {
      for (const dir of ["scripts", "tomcat", "bin", "caller", "home", "custom target"]) {
        fs.mkdirSync(path.join(fixture, dir));
      }
      fs.copyFileSync(path.join(root, "scripts/dev-install.cmd"), path.join(fixture, "scripts/dev-install.cmd"));
      const log = path.join(fixture, "cargo.jsonl");
      const restored = path.join(fixture, "restored.txt");
      // Invoke the real batch wrapper, replacing only Cargo (no build or installation in this test).
      fs.writeFileSync(path.join(fixture, "bin/cargo.cmd"), '@echo off\r\n"%TEST_NODE%" "%~dp0cargo.cjs" %*\r\nexit /b %errorlevel%\r\n');
      fs.writeFileSync(path.join(fixture, "bin/cargo.cjs"), `
const fs = require("node:fs");
fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({args:process.argv.slice(2), cwd:process.cwd(), target:process.env.CARGO_TARGET_DIR, home:process.env.HOME, active:process.env.TOMCAT_AGENT_ACTIVE}) + "\\n");
process.exit(Number(process.env.TEST_EXIT));
`);
      fs.writeFileSync(path.join(fixture, "caller/check.cmd"), '@echo off\r\ncall "..\\scripts\\dev-install.cmd"\r\nset "saved=%errorlevel%"\r\n"%TEST_NODE%" -e "require(\'fs\').writeFileSync(process.env.TEST_RESTORED, process.cwd())"\r\nexit /b %saved%\r\n');
      // Remove case-duplicates of PATH before assigning it on Windows.
      const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PATH"));
      Object.assign(env, {
        PATH: `${path.join(fixture, "bin")};${process.env.PATH ?? ""}`,
        TEST_NODE: process.execPath, TEST_LOG: log, TEST_EXIT: String(exitCode), TEST_RESTORED: restored,
        CARGO_TARGET_DIR: path.join(fixture, "custom target"), HOME: path.join(fixture, "home"), TOMCAT_AGENT_ACTIVE: "1",
      });
      const result = spawnSync(process.env.ComSpec ?? "cmd.exe", ["/d", "/c", "check.cmd"], {
        cwd: path.join(fixture, "caller"), env, encoding: "utf8", windowsHide: true,
      });
      assert.equal(result.status, exitCode, `${result.error ?? ""}\n${result.stdout}\n${result.stderr}`);
      const entries = fs.readFileSync(log, "utf8").trim().split(/\r?\n/).map(JSON.parse);
      assert.deepEqual(entries, [{ args: ["run", "--release", "--bin", "tomcat", "--", "self-install"],
        cwd: path.join(fixture, "tomcat"), target: env.CARGO_TARGET_DIR, home: env.HOME, active: "1" }]);
      assert.equal(fs.readFileSync(restored, "utf8"), path.join(fixture, "caller"));
      assert.deepEqual(fs.readdirSync(env.HOME), []);
    } finally {
      fs.rmSync(fixture, { recursive: true, force: true });
    }
  });
}
