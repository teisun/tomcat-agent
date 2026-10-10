import { t } from "../../../../src/shared/i18n";
import type { WebviewMediaRoot } from "../../types";

const WINDOWS_DRIVE_RE = /^[a-z]:[\\/]/iu;
const URI_SCHEME_RE = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/iu;

function hasUriScheme(value: string): boolean {
  return !WINDOWS_DRIVE_RE.test(value) && URI_SCHEME_RE.test(value);
}

function toForwardSlash(value: string): string {
  return value.replaceAll("\\", "/");
}

function normalizeAbsolutePath(value: string): string {
  const slashValue = toForwardSlash(value);
  const driveMatch = slashValue.match(/^([a-z]):(\/.*)?$/iu);
  const drivePrefix = driveMatch ? `${driveMatch[1].toLowerCase()}:` : "";
  const rest = driveMatch ? driveMatch[2] ?? "/" : slashValue;
  const isAbsolute = drivePrefix !== "" || rest.startsWith("/");
  const segments: string[] = [];
  for (const segment of rest.split("/")) {
    if (!segment || segment === ".") {
      continue;
    }
    if (segment === "..") {
      if (segments.length > 0) {
        segments.pop();
      }
      continue;
    }
    segments.push(segment);
  }
  const normalizedRest = isAbsolute ? `/${segments.join("/")}` : segments.join("/");
  if (drivePrefix) {
    return normalizedRest === "/" ? `${drivePrefix}/` : `${drivePrefix}${normalizedRest}`;
  }
  return normalizedRest || "/";
}

function canonicalizeFsPath(value: string): string {
  let normalized = normalizeAbsolutePath(value);
  if (normalized.startsWith("/private/tmp/") || normalized === "/private/tmp") {
    normalized = normalized.slice("/private".length);
  } else if (normalized.startsWith("/private/var/") || normalized === "/private/var") {
    normalized = normalized.slice("/private".length);
  }
  return normalized;
}

function isWithinRoot(candidatePath: string, rootPath: string): boolean {
  return candidatePath === rootPath || candidatePath.startsWith(`${rootPath}/`);
}

function encodeRelativePath(value: string): string {
  return value
    .split(/[\\/]+/u)
    .filter((segment) => segment.length > 0)
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

function joinWebviewBase(base: string, relativePath: string): string {
  const trimmedBase = base.replace(/\/+$/u, "");
  const encoded = encodeRelativePath(relativePath);
  return encoded ? `${trimmedBase}/${encoded}` : trimmedBase;
}

function resolveAgainstRoot(
  rawSrc: string,
  root: WebviewMediaRoot,
): string | null {
  const rootPath = canonicalizeFsPath(root.fsPath);
  const candidatePath = canonicalizeFsPath(
    rawSrc.startsWith("/") || WINDOWS_DRIVE_RE.test(rawSrc)
      ? rawSrc
      : `${root.fsPath}/${rawSrc}`,
  );
  if (!isWithinRoot(candidatePath, rootPath)) {
    return null;
  }
  const relativePath = candidatePath.slice(rootPath.length).replace(/^\/+/u, "");
  if (!relativePath) {
    return null;
  }
  return joinWebviewBase(root.webviewBase, relativePath);
}

export function resolveLocalImageSrc(
  rawSrc: string,
  roots: WebviewMediaRoot[],
): string | null {
  const trimmed = rawSrc.trim();
  if (!trimmed || hasUriScheme(trimmed)) {
    return null;
  }
  for (const root of roots) {
    const resolved = resolveAgainstRoot(trimmed, root);
    if (resolved) {
      return resolved;
    }
  }
  return null;
}

function createBlockedImageReplacement(
  image: HTMLImageElement,
  rawSrc: string,
): HTMLAnchorElement {
  const link = image.ownerDocument.createElement("a");
  link.className = "tc-blocked-image";
  link.dataset.testid = "blocked-inline-image";
  const suppliedLabel = rawSrc || image.getAttribute("alt");
  if (!suppliedLabel) link.dataset.tcDefaultImageLabel = "1";
  const label = suppliedLabel || t("image.label");
  link.textContent = label;
  link.title = label;
  if (hasUriScheme(rawSrc)) {
    link.href = rawSrc;
  } else {
    link.href = "#";
    link.dataset.tcFilePath = rawSrc;
  }
  return link;
}

export function rewriteLocalImages(
  container: HTMLElement,
  roots: WebviewMediaRoot[],
): void {
  for (const image of [...container.querySelectorAll<HTMLImageElement>("img")]) {
    const rawSrc = image.getAttribute("src")?.trim() ?? "";
    const resolved = resolveLocalImageSrc(rawSrc, roots);
    if (resolved) {
      image.classList.add("tc-inline-image");
      image.dataset.tcImageSrc = resolved;
      image.dataset.testid = "inline-image";
      image.src = resolved;
      continue;
    }
    image.replaceWith(createBlockedImageReplacement(image, rawSrc));
  }
}
