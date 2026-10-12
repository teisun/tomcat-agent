import path from "node:path";
import { access, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

export function scriptDirectory(metaUrl) {
  return path.dirname(fileURLToPath(metaUrl));
}

export function resolveWorkDir(metaUrl) {
  return path.resolve(scriptDirectory(metaUrl), "../../..");
}

export function resolveBrowserPath(metaUrl, env = process.env) {
  const configured = env.PLAYWRIGHT_BROWSERS_PATH;
  return configured
    ? path.resolve(configured)
    : path.join(resolveWorkDir(metaUrl), "cache", "playwright");
}

export function systemBrowserCandidates(platform = process.platform) {
  if (platform === "darwin") {
    return [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
  }
  if (platform === "win32") {
    return [
      "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
      "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    ];
  }
  return ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
}

export async function findSystemBrowser() {
  for (const candidate of systemBrowserCandidates()) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next known installation path.
    }
  }
  return undefined;
}

export async function resolveLaunchOptions(metaUrl, env = process.env) {
  if (env.PLAYWRIGHT_BROWSER_EXECUTABLE_PATH) {
    return { executablePath: path.resolve(env.PLAYWRIGHT_BROWSER_EXECUTABLE_PATH) };
  }
  const markerPath = path.join(resolveBrowserPath(metaUrl, env), "system-browser.json");
  try {
    const marker = JSON.parse(await readFile(markerPath, "utf8"));
    if (typeof marker.executablePath === "string") {
      await access(marker.executablePath);
      return { executablePath: marker.executablePath };
    }
  } catch {
    // No valid fallback marker: launch Playwright's managed browser as usual.
  }
  return {};
}
