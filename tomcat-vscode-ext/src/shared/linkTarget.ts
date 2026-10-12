/**
 * A Markdown link target after separating URL navigation from a local file
 * reference. Relative-file resolution deliberately stays with each caller:
 * chat is relative to the workspace, whereas a plan preview is relative to
 * the plan file that contains the link.
 */
export type LinkTarget =
  | { href: string; kind: "external" }
  | { kind: "file"; line?: number; path: string }
  | { kind: "ignore" };

function hasUriScheme(value: string): boolean {
  // A Windows drive is a local path, not a URI scheme.
  if (/^[a-z]:[\\/]/iu.test(value)) {
    return false;
  }
  return /^[a-z][a-z0-9+.-]*:/iu.test(value);
}

/**
 * Remove a supported source-location suffix from a local path. End locations
 * remain in the visible Markdown label, but opening a file only needs its
 * first line.
 */
export function splitPathLocation(value: string): { line?: number; path: string } {
  const trimmed = value.trim();
  const hashMatch = trimmed.match(/^(.*)#L(\d+)(?:C(\d+))?(?:-L?(\d+)(?:C(\d+))?)?$/u);
  if (hashMatch) {
    return { line: Number(hashMatch[2]), path: hashMatch[1] };
  }
  const colonMatch = trimmed.match(/^(.*?):(\d+)(?::(\d+))?(?:-(\d+)(?::(\d+))?)?$/u);
  if (colonMatch && colonMatch[1].length > 1) {
    return { line: Number(colonMatch[2]), path: colonMatch[1] };
  }
  return { path: trimmed.split("#", 1)[0] };
}

export function classifyLink(href: string): LinkTarget {
  const trimmed = href.trim();
  if (!trimmed || trimmed.startsWith("#")) {
    return { kind: "ignore" };
  }
  if (hasUriScheme(trimmed)) {
    return { href: trimmed, kind: "external" };
  }
  const location = splitPathLocation(trimmed);
  return location.path ? { kind: "file", ...location } : { kind: "ignore" };
}
