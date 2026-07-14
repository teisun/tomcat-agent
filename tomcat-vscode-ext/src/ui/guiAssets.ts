import * as fs from "node:fs";
import * as path from "node:path";

/**
 * Assets a webview entry needs, resolved to absolute paths under gui/dist.
 *
 * The single source of truth is the entry HTML that vite emits (index.html /
 * plan.html / settings.html). Reading it means any file vite decides to ship
 * (extra CSS such as codicon.css, split chunks, ...) is carried into the
 * hand-rolled webview HTML automatically, instead of a hand-maintained list
 * that silently drifts from the build output.
 */
export interface WebviewEntryAssets {
  /** Absolute paths of entry `<script type="module">` sources. */
  scripts: string[];
  /** Absolute paths of `<link rel="stylesheet">` hrefs. */
  stylesheets: string[];
}

const LINK_TAG_RE = /<link\b[^>]*>/gi;
const SCRIPT_TAG_RE = /<script\b[^>]*>/gi;
const HREF_RE = /\bhref\s*=\s*["']([^"']+)["']/i;
const SRC_RE = /\bsrc\s*=\s*["']([^"']+)["']/i;
const REL_STYLESHEET_RE = /\brel\s*=\s*["']stylesheet["']/i;
const TYPE_MODULE_RE = /\btype\s*=\s*["']module["']/i;

function isLocalReference(ref: string): boolean {
  return !/^[a-z][a-z0-9+.-]*:\/\//i.test(ref) && !ref.startsWith("data:");
}

function resolveExisting(distRoot: string, ref: string): string | null {
  if (!isLocalReference(ref)) {
    return null;
  }
  const withoutQuery = ref.split("?")[0].split("#")[0];
  const relative = withoutQuery.replace(/^\.?\//, "");
  if (!relative) {
    return null;
  }
  const absolute = path.join(distRoot, relative);
  return fs.existsSync(absolute) ? absolute : null;
}

function parseEntryHtml(distRoot: string, html: string): WebviewEntryAssets {
  const stylesheets: string[] = [];
  for (const tag of html.match(LINK_TAG_RE) ?? []) {
    if (!REL_STYLESHEET_RE.test(tag)) {
      continue;
    }
    const href = tag.match(HREF_RE)?.[1];
    if (!href) {
      continue;
    }
    const absolute = resolveExisting(distRoot, href);
    if (absolute && !stylesheets.includes(absolute)) {
      stylesheets.push(absolute);
    }
  }

  const scripts: string[] = [];
  for (const tag of html.match(SCRIPT_TAG_RE) ?? []) {
    if (!TYPE_MODULE_RE.test(tag)) {
      continue;
    }
    const src = tag.match(SRC_RE)?.[1];
    if (!src) {
      continue;
    }
    const absolute = resolveExisting(distRoot, src);
    if (absolute && !scripts.includes(absolute)) {
      scripts.push(absolute);
    }
  }

  return { scripts, stylesheets };
}

/** All `.css` files in dist, with `styles.css` first for deterministic order. */
export function resolveAllStylesheets(distRoot: string): string[] {
  try {
    return fs
      .readdirSync(distRoot, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".css"))
      .map((entry) => path.join(distRoot, entry.name))
      .sort((left, right) => {
        const leftName = path.basename(left);
        const rightName = path.basename(right);
        if (leftName === "styles.css") {
          return -1;
        }
        if (rightName === "styles.css") {
          return 1;
        }
        return leftName.localeCompare(rightName);
      });
  } catch {
    return [];
  }
}

/**
 * Resolve every asset a webview entry declares, preferring the vite-emitted
 * entry HTML as the source of truth and falling back to a best-effort glob
 * (all `.css` + the known entry script) when the HTML is missing.
 */
export function resolveWebviewEntryAssets(
  distRoot: string,
  entryHtmlName: string,
  fallbackScriptName: string,
): WebviewEntryAssets {
  const htmlPath = path.join(distRoot, entryHtmlName);
  if (fs.existsSync(htmlPath)) {
    try {
      const parsed = parseEntryHtml(distRoot, fs.readFileSync(htmlPath, "utf8"));
      if (parsed.scripts.length > 0) {
        return parsed;
      }
    } catch {
      // Fall through to the glob-based fallback below.
    }
  }

  const scriptPath = path.join(distRoot, fallbackScriptName);
  return {
    scripts: fs.existsSync(scriptPath) ? [scriptPath] : [],
    stylesheets: resolveAllStylesheets(distRoot),
  };
}
