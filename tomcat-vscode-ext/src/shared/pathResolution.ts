/**
 * The host decides whether a free-form path token can safely become a clickable
 * file chip. Webviews receive the original token plus the absolute target to
 * open, so each surface can preserve its own relative-path semantics.
 */
export type ResolvedPathKind = "directory" | "file" | "missing";

export interface PathResolution {
  kind: ResolvedPathKind;
  /** The candidate path supplied by the webview, without a line-location suffix. */
  path: string;
  /** Absolute path opened by the host when `kind` is file or directory. */
  resolvedPath: string;
}
