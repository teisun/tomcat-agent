#!/usr/bin/env node

import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { resolveBrowserPath, resolveLaunchOptions } from "./browser-path.mjs";

const DEFAULT_TIMEOUT_MS = 30_000;
const DEFAULT_VIEWPORT = { width: 1440, height: 900 };
const SAFE_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

function usage(message) {
  if (message) {
    console.error(`shot.mjs: ${message}`);
  }
  console.error(
    "Usage: node shot.mjs <url> [--out <directory>] [--name <name>] [--viewport <width>x<height>] [--wait <networkidle|selector>] [--timeout-ms <ms>]",
  );
  process.exitCode = 2;
}

function parseViewport(value) {
  const match = /^([1-9]\d{0,4})x([1-9]\d{0,4})$/.exec(value);
  if (!match) {
    throw new Error(`Invalid viewport "${value}"; expected WIDTHxHEIGHT.`);
  }
  const width = Number(match[1]);
  const height = Number(match[2]);
  if (width > 10_000 || height > 10_000) {
    throw new Error("Viewport dimensions must be at most 10000.");
  }
  return { width, height };
}

function parseArguments(argv) {
  const options = {
    name: "page",
    viewport: DEFAULT_VIEWPORT,
    wait: "networkidle",
    timeoutMs: DEFAULT_TIMEOUT_MS,
    out: path.resolve(".agents", "shots"),
  };
  const [url, ...rest] = argv;
  if (!url) {
    usage("Missing URL.");
    return undefined;
  }
  try {
    const parsed = new URL(url);
    if (!["http:", "https:"].includes(parsed.protocol)) {
      throw new Error("URL must use http or https.");
    }
  } catch (error) {
    usage(error.message);
    return undefined;
  }

  for (let index = 0; index < rest.length; index += 1) {
    const flag = rest[index];
    const value = rest[index + 1];
    if (!["--out", "--name", "--viewport", "--wait", "--timeout-ms"].includes(flag)) {
      usage(`Unknown option "${flag}".`);
      return undefined;
    }
    if (!value || value.startsWith("--")) {
      usage(`Missing value for "${flag}".`);
      return undefined;
    }
    index += 1;
    if (flag === "--out") options.out = path.resolve(value);
    if (flag === "--name") options.name = value;
    if (flag === "--viewport") options.viewport = parseViewport(value);
    if (flag === "--wait") options.wait = value;
    if (flag === "--timeout-ms") {
      options.timeoutMs = Number(value);
      if (!Number.isSafeInteger(options.timeoutMs) || options.timeoutMs <= 0) {
        throw new Error("--timeout-ms must be a positive integer.");
      }
    }
  }

  if (!SAFE_NAME.test(options.name)) {
    usage("--name may contain only letters, digits, dot, underscore, and hyphen.");
    return undefined;
  }
  if (options.wait !== "networkidle" && !options.wait.startsWith("selector:")) {
    usage('--wait must be "networkidle" or "selector:<css-selector>".');
    return undefined;
  }
  return { url, ...options };
}

function consoleLocation(message) {
  const location = message.location();
  return location.url
    ? { url: location.url, lineNumber: location.lineNumber, columnNumber: location.columnNumber }
    : undefined;
}

function bootstrapCommand() {
  return `node ${fileURLToPath(new URL("./bootstrap.mjs", import.meta.url))}`;
}

async function loadChromium() {
  try {
    const { chromium } = await import("playwright");
    return chromium;
  } catch (error) {
    throw new Error(
      `Playwright dependencies are unavailable: ${error.message}\nRun ${bootstrapCommand()} first.`,
    );
  }
}

async function launchBrowser(chromium) {
  try {
    return await chromium.launch({
      headless: true,
      ...(await resolveLaunchOptions(import.meta.url)),
    });
  } catch (error) {
    throw new Error(
      `Playwright browser is unavailable: ${error.message}\nRun ${bootstrapCommand()} first.`,
    );
  }
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  if (!options) return;

  await mkdir(options.out, { recursive: true });
  const pngPath = path.join(options.out, `${options.name}.png`);
  const ariaPath = path.join(options.out, `${options.name}.aria.txt`);
  const consolePath = path.join(options.out, `${options.name}.console.json`);
  const browserPath = resolveBrowserPath(import.meta.url);
  process.env.PLAYWRIGHT_BROWSERS_PATH = browserPath;
  const chromium = await loadChromium();
  const events = [];

  let browser;
  try {
    browser = await launchBrowser(chromium);
    const context = await browser.newContext({ viewport: options.viewport });
    const page = await context.newPage();
    page.on("console", (message) => {
      events.push({
        kind: "console",
        level: message.type(),
        text: message.text(),
        location: consoleLocation(message),
      });
    });
    page.on("pageerror", (error) => {
      events.push({ kind: "pageerror", level: "error", text: error.message });
    });

    await page.goto(options.url, {
      waitUntil: "domcontentloaded",
      timeout: options.timeoutMs,
    });
    if (options.wait === "networkidle") {
      await page.waitForLoadState("networkidle", { timeout: options.timeoutMs });
    } else {
      await page.waitForSelector(options.wait.slice("selector:".length), {
        state: "visible",
        timeout: options.timeoutMs,
      });
    }

    await page.screenshot({ path: pngPath, fullPage: true });
    const ariaSnapshot = await page.locator("body").ariaSnapshot();
    await writeFile(ariaPath, `${ariaSnapshot}\n`, "utf8");
    await writeFile(
      consolePath,
      `${JSON.stringify(
        {
          url: page.url(),
          viewport: options.viewport,
          events,
        },
        null,
        2,
      )}\n`,
      "utf8",
    );

    const errors = events.filter(
      (event) => event.kind === "pageerror" || event.level === "error",
    );
    if (errors.length > 0) {
      throw new Error(
        `Captured ${errors.length} browser error(s); see ${consolePath}.`,
      );
    }
    console.log(
      `[verify shot] wrote ${pngPath}, ${ariaPath}, and ${consolePath}`,
    );
  } finally {
    await browser?.close();
  }
}

main().catch((error) => {
  console.error(`[verify shot] ${error.message}`);
  process.exitCode = 1;
});
