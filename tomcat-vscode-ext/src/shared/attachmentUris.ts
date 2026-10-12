/**
 * Hash-to-URI mapping — how images reach a webview without any bytes in JavaScript.
 *
 * ## The problem this solves
 *
 * A `data:` URI puts an image's base64 text on the JavaScript heap and *also* makes
 * Chromium decode a bitmap. Both copies are pinned as long as the string is reachable,
 * and neither can be evicted under memory pressure. Eleven 4.5MB photos in a composer
 * came to roughly half a gigabyte that way.
 *
 * `asWebviewUri` turns a file path into a URL the webview can fetch. The bytes travel
 * over VS Code's resource protocol straight into Chromium's image cache. Nothing lands
 * on the JS heap, and Chromium is free to drop the bitmap when memory gets tight.
 *
 * ```
 *   before                                  after
 *   ------                                  -----
 *   read file -> base64 -> postMessage      send a URL string
 *   -> JS string (6MB) + bitmap (48MB)      -> bitmap only, evictable
 *   -> pinned for the session's lifetime    -> ~80 byte string on the heap
 * ```
 *
 * ## Why there is no base64 fallback
 *
 * If `localResourceRoots` is misconfigured the image fails to load, and that failure is
 * left visible on purpose. A silent fallback to `data:` URIs would restore exactly the
 * memory profile this exists to eliminate, and would do it invisibly — the bug would
 * ship, work fine in a small manual test, and reappear as an out-of-memory report from
 * whoever pastes eleven photos.
 */
import * as vscode from "vscode";

/**
 * Subdirectories of the backend attachment root a webview may read from.
 *
 * `pending/` is deliberately absent: it holds lease markers, not bytes, and nothing in
 * the UI has any business reading it.
 */
const READABLE_SUBDIRS = ["blobs", "thumbs"] as const;

/** A webview-facing pair of URLs for one attachment. */
export interface AttachmentUris {
  /** Full-resolution source. Only the preview panel should load this. */
  fullUri: string;
  /**
   * Downsampled source, or null when no thumbnail has been generated yet.
   *
   * Deliberately not falling back to {@link fullUri}. That fallback reads as harmless —
   * "correct, just heavier" — but it is how a strip of eleven 4000x3000 photos came to
   * cost 480MB of decoded bitmaps: any attachment that did not arrive through the paste
   * path had no thumbnail, so every one of them silently decoded at full size. A null
   * here means the strip shows a placeholder and the webview generates the thumbnail it
   * is missing, which is bounded no matter how the image got there.
   */
  thumbUri: string | null;
}

function isSha256(value: string): boolean {
  return /^[0-9a-f]{64}$/.test(value);
}

/**
 * Directories a webview needs read access to in order to display attachments.
 *
 * The attachment root sits in the user's tomcat data directory, nowhere near
 * `extensionUri`, so access has to be granted explicitly.
 */
export function attachmentResourceRoots(attachmentRoot: string | null): vscode.Uri[] {
  if (!attachmentRoot) return [];
  const root = vscode.Uri.file(attachmentRoot);
  return READABLE_SUBDIRS.map((subdir) => vscode.Uri.joinPath(root, subdir));
}

/**
 * Resolve one attachment's hash into webview URLs.
 *
 * A thumbnail is addressed by the hash of the image it came from, never by its own, so
 * the whole UI has exactly one lookup rule:
 *
 * ```
 *   full image   blobs/<blobSha>
 *   thumbnail    thumbs/<blobSha>
 * ```
 *
 * History images land in `blobs/` too, rebuilt from the transcript on demand. An earlier
 * draft gave them a separate `cache/` directory, which meant a caller holding a hash had
 * to guess which of two directories it was in — for no gain, since both hold the same
 * immutable bytes under the same name.
 *
 * Returns null for an unknown root or a malformed hash, so a caller cannot accidentally
 * emit a URL pointing outside the blob store.
 */
export function resolveAttachmentUris(
  webview: Pick<vscode.Webview, "asWebviewUri">,
  attachmentRoot: string | null,
  attachment: { blobSha: string; hasThumb?: boolean | null },
): AttachmentUris | null {
  if (!attachmentRoot || !isSha256(attachment.blobSha)) {
    return null;
  }
  const root = vscode.Uri.file(attachmentRoot);
  const uriFor = (...parts: string[]): string =>
    webview.asWebviewUri(vscode.Uri.joinPath(root, ...parts)).toString();

  return {
    fullUri: uriFor("blobs", attachment.blobSha),
    thumbUri: attachment.hasThumb ? uriFor("thumbs", attachment.blobSha) : null,
  };
}
