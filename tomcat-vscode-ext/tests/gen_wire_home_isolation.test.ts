import { execFile } from "node:child_process";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import { expect, it } from "vitest";
import { resolveCargoCommand } from "../scripts/resolveCargoCommand";

it("exports the real schema without modifying the caller HOME", async () => {
  const callerHome = await mkdtemp(path.join(os.tmpdir(), "schema-caller-home-"));
  const callerData = path.join(callerHome, ".tomcat");
  const originalHome = os.homedir();
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (key.startsWith("TOMCAT__") || /_API_KEY$/i.test(key)) delete env[key];
  }
  Object.assign(env, {
    HOME: callerHome, USERPROFILE: callerHome,
    CARGO: resolveCargoCommand(),
    CARGO_HOME: process.env.CARGO_HOME ?? path.join(originalHome, ".cargo"),
    RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(originalHome, ".rustup"),
    TOMCAT__STORAGE__WORK_DIR: callerData,
  });
  try {
    await mkdir(callerData);
    await writeFile(path.join(callerData, "keep"), "caller-owned data");
    await promisify(execFile)(process.execPath, ["--import", "tsx", "scripts/gen-wire.ts", "--check"], {
      cwd: path.resolve(__dirname, ".."), env, timeout: 240_000,
    });
    expect(await readdir(callerHome)).toEqual([".tomcat"]);
    expect(await readdir(callerData)).toEqual(["keep"]);
    expect(await readFile(path.join(callerData, "keep"), "utf8")).toBe("caller-owned data");
  } finally {
    await rm(callerHome, { recursive: true, force: true });
  }
}, 300_000);
