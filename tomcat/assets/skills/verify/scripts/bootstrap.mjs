#!/usr/bin/env node

import { spawn } from "node:child_process";
import { access, mkdir, unlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  resolveBrowserPath,
  scriptDirectory,
  findSystemBrowser,
} from "./browser-path.mjs";

function run(command, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: "inherit", ...options });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
      } else {
        reject(
          new Error(
            `${command} ${args.join(" ")} exited with ${
              signal ? `signal ${signal}` : `code ${code}`
            }`,
          ),
        );
      }
    });
  });
}

async function ensureFile(filePath, message) {
  try {
    await access(filePath);
  } catch {
    throw new Error(message);
  }
}

async function main() {
  const scriptsDir = scriptDirectory(import.meta.url);
  const browserPath = resolveBrowserPath(import.meta.url);
  const fallbackMarker = path.join(browserPath, "system-browser.json");
  const npmCommand = process.platform === "win32" ? "npm.cmd" : "npm";
  const packageLock = path.join(scriptsDir, "package-lock.json");

  await ensureFile(
    packageLock,
    `Missing managed package lock: ${packageLock}. Reinstall or repair the verify skill.`,
  );
  await mkdir(browserPath, { recursive: true });

  console.log(`[verify bootstrap] installing locked Node dependencies in ${scriptsDir}`);
  await run(npmCommand, ["ci"], {
    cwd: scriptsDir,
    env: {
      ...process.env,
      PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD: "1",
    },
  });

  const playwrightCli = path.join(scriptsDir, "node_modules", "playwright", "cli.js");
  await ensureFile(
    playwrightCli,
    `npm ci completed without the Playwright CLI at ${playwrightCli}`,
  );

  const browserEnv = { ...process.env, PLAYWRIGHT_BROWSERS_PATH: browserPath };
  delete browserEnv.PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD;
  console.log(`[verify bootstrap] installing Chromium in ${browserPath}`);
  try {
    await run(process.execPath, [playwrightCli, "install", "chromium"], {
      cwd: scriptsDir,
      env: browserEnv,
    });
    await unlink(fallbackMarker).catch(() => {});
  } catch (error) {
    const macosMajor = Number.parseInt(os.release().split(".")[0], 10) - 9;
    const bundledChromiumUnsupported =
      process.platform === "darwin" && Number.isFinite(macosMajor) && macosMajor < 14;
    if (!bundledChromiumUnsupported) {
      throw error;
    }
    const systemBrowser = await findSystemBrowser();
    if (!systemBrowser) {
      throw new Error(
        `${error.message}. This platform cannot use Playwright's bundled Chromium and no supported system Chrome/Chromium was found. Set PLAYWRIGHT_BROWSER_EXECUTABLE_PATH to a compatible browser.`,
      );
    }
    await writeFile(
      fallbackMarker,
      `${JSON.stringify({ executablePath: systemBrowser }, null, 2)}\n`,
      "utf8",
    );
    console.warn(
      `[verify bootstrap] bundled Chromium is unavailable on this platform; using ${systemBrowser}`,
    );
  }

  console.log("[verify bootstrap] ready");
}

main().catch((error) => {
  console.error(`[verify bootstrap] ${error.message}`);
  process.exitCode = 1;
});
