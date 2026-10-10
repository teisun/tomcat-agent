/**
 * Webview-side image pipeline — the only place in the product that touches pixels.
 *
 * Why here and not in Rust or the extension host: a webview *is* Chromium. It already
 * has a decoder, a resampler and an SVG renderer, all of them memory-managed by the
 * browser. Doing this work here costs zero dependencies; doing it anywhere else costs
 * either 44 Rust crates or a per-platform native module.
 *
 * Two jobs:
 *
 * 1. Downsample to a thumbnail at decode time. `createImageBitmap` with `resizeWidth`
 *    lets Chromium downsample *during* decoding, so a 4000x3000 JPEG never
 *    materialises as a 48MB bitmap. Decode-then-resize — which is what an image
 *    library on the Rust side would do — has to pay that 48MB first.
 *
 * 2. Rasterise SVG to PNG for the model. Providers do not accept `image/svg+xml`.
 *    Rendering happens through `<img>`, which the HTML spec puts in *secure static
 *    mode*: no scripts, no external fetches. That is a spec-level guarantee, unlike
 *    the string blacklist this replaces.
 *
 * Every step degrades rather than fails. Display never depends on any of it — the
 * original bytes are always what gets shown.
 */

import { t } from "../../../src/shared/i18n";

/** Longest edge of a generated thumbnail, in CSS pixels. */
export const THUMBNAIL_MAX_EDGE = 192;

/**
 * Upper bound on SVG source we are willing to inline as text for the model.
 *
 * Past this size the source stops being cheaper than a raster and starts eating the
 * context window.
 */
export const SVG_SOURCE_MAX_BYTES = 50 * 1024;

/** Fallback canvas size when an SVG declares no intrinsic dimensions. */
const SVG_FALLBACK_SIZE = 512;

/**
 * How long to wait for a decode before giving up on it.
 *
 * `<img>` loading has a third outcome besides load and error: nothing. A malformed SVG
 * can leave the element sitting there with neither event ever firing, and without a
 * bound the promise chain behind a paste never settles — the user's images simply never
 * appear, with no error to explain it. Timing out degrades to "no thumbnail", which the
 * caller already knows how to handle.
 */
const DECODE_TIMEOUT_MS = 10_000;

export interface PreparedAttachment {
  /** Original bytes, base64 without any `data:` prefix. Always what gets displayed. */
  dataBase64: string;
  filename: string | null;
  mimeType: string;
  /** Original local path, when Chromium exposed one on the File. */
  sourcePath?: string | null;
  /** Downsampled preview, base64. Absent when generation failed. */
  thumbBase64?: string;
  /** Provider-friendly rendering (PNG) for formats providers reject, base64. */
  providerBase64?: string;
  providerMimeType?: string;
  /**
   * SVG source to hand the model as text, used when rasterisation failed.
   *
   * Often better than a raster for logos and icons: the model reads exact shapes,
   * colour values and label text instead of guessing at pixels.
   */
  providerText?: string;
  /** Non-fatal notes worth surfacing to the user. */
  warnings: string[];
}

/** Bytes plus enough metadata to decode them. */
export interface RawAttachment {
  bytes: ArrayBuffer;
  filename: string | null;
  mimeType: string;
  sourcePath?: string | null;
}

function toBase64(bytes: ArrayBuffer | Uint8Array): string {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  // Chunked to stay clear of the argument-count limit on large images.
  const chunkSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < view.length; offset += chunkSize) {
    binary += String.fromCharCode(...view.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

export async function blobToBase64(blob: Blob): Promise<string> {
  return toBase64(await blob.arrayBuffer());
}

/**
 * Read an SVG's intrinsic size, filling in a square fallback when it declares none.
 *
 * A great many real-world SVGs carry only a `viewBox`, or nothing at all. Chromium
 * then reports `naturalWidth === 0` and `drawImage` silently produces a blank canvas.
 * Stamping explicit dimensions onto the root element before loading is what makes
 * rasterisation reliable across the SVGs people actually paste.
 */
export function svgWithExplicitSize(source: string): {
  height: number;
  source: string;
  width: number;
} {
  const widthMatch = /<svg[^>]*?\swidth\s*=\s*["']([\d.]+)/i.exec(source);
  const heightMatch = /<svg[^>]*?\sheight\s*=\s*["']([\d.]+)/i.exec(source);
  const viewBoxMatch =
    /<svg[^>]*?\sviewBox\s*=\s*["']\s*[-\d.]+[,\s]+[-\d.]+[,\s]+([\d.]+)[,\s]+([\d.]+)/i.exec(
      source,
    );

  let width = widthMatch ? Number(widthMatch[1]) : 0;
  let height = heightMatch ? Number(heightMatch[1]) : 0;
  if ((!width || !height) && viewBoxMatch) {
    width = width || Number(viewBoxMatch[1]);
    height = height || Number(viewBoxMatch[2]);
  }
  if (!width || !height || !Number.isFinite(width) || !Number.isFinite(height)) {
    width = SVG_FALLBACK_SIZE;
    height = SVG_FALLBACK_SIZE;
  }

  if (widthMatch && heightMatch) {
    return { height, source, width };
  }
  // Inject the dimensions we just derived so the browser has an intrinsic size.
  const patched = source.replace(/<svg\b/i, `<svg width="${width}" height="${height}"`);
  return { height, source: patched, width };
}

/** Scale a natural size down so its longest edge is at most `maxEdge`. */
export function fitWithin(
  width: number,
  height: number,
  maxEdge: number,
): { height: number; width: number } {
  const longest = Math.max(width, height);
  if (longest <= maxEdge || longest === 0) {
    return {
      height: Math.max(1, Math.round(height)),
      width: Math.max(1, Math.round(width)),
    };
  }
  const scale = maxEdge / longest;
  return {
    height: Math.max(1, Math.round(height * scale)),
    width: Math.max(1, Math.round(width * scale)),
  };
}

/**
 * Load a blob through `<img>` and hand back the element plus its resolved size.
 *
 * `<img>` rather than `createImageBitmap` because it is the only path that renders
 * SVG, and because it is the path the spec pins to secure static mode.
 */
async function loadImageElement(
  blob: Blob,
  hint?: { height: number; width: number },
): Promise<{ height: number; image: HTMLImageElement; width: number }> {
  const url = URL.createObjectURL(blob);
  try {
    const image = new Image();
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("image decode timed out")),
        DECODE_TIMEOUT_MS,
      );
      image.onload = () => {
        clearTimeout(timer);
        resolve();
      };
      image.onerror = () => {
        clearTimeout(timer);
        reject(new Error("image failed to load"));
      };
      image.src = url;
    });
    const width = image.naturalWidth || hint?.width || SVG_FALLBACK_SIZE;
    const height = image.naturalHeight || hint?.height || SVG_FALLBACK_SIZE;
    return { height, image, width };
  } finally {
    URL.revokeObjectURL(url);
  }
}

function canvasToBlob(
  canvas: HTMLCanvasElement,
  mimeType: string,
  quality?: number,
): Promise<Blob> {
  return new Promise((resolve, reject) => {
    try {
      canvas.toBlob(
        (blob) => (blob ? resolve(blob) : reject(new Error("canvas produced no blob"))),
        mimeType,
        quality,
      );
    } catch (error) {
      // A tainted canvas throws SecurityError here rather than passing null. Same-origin
      // blob: URLs should never taint, but this is the documented failure mode, so it
      // has to be caught rather than assumed away.
      reject(error instanceof Error ? error : new Error(String(error)));
    }
  });
}

function drawToCanvas(
  source: CanvasImageSource,
  width: number,
  height: number,
): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (!context) {
    throw new Error("2d canvas context unavailable");
  }
  context.drawImage(source, 0, 0, width, height);
  return canvas;
}

/**
 * Rasterise SVG bytes to PNG using nothing but the browser.
 *
 * Throws on failure so the caller can fall back to sending the source as text.
 */
export async function rasterizeSvgToPng(bytes: ArrayBuffer): Promise<Blob> {
  const sized = svgWithExplicitSize(new TextDecoder("utf-8").decode(bytes));
  const loaded = await loadImageElement(
    new Blob([sized.source], { type: "image/svg+xml" }),
    sized,
  );
  return canvasToBlob(
    drawToCanvas(loaded.image, loaded.width, loaded.height),
    "image/png",
  );
}

/**
 * Produce a thumbnail whose longest edge is at most {@link THUMBNAIL_MAX_EDGE}.
 *
 * Prefers `createImageBitmap` with both resize dimensions so the downsampling happens
 * inside the decoder. Falls back to draw-then-export where that is unsupported —
 * notably for SVG, which `createImageBitmap` does not accept.
 */
export async function makeThumbnail(
  bytes: ArrayBuffer,
  mimeType: string,
): Promise<Blob> {
  const blob = new Blob([bytes], { type: mimeType });

  if (mimeType !== "image/svg+xml" && typeof createImageBitmap === "function") {
    // Probe the natural size first so the aspect ratio survives. Passing resizeWidth
    // alone would squash anything portrait-shaped.
    const probe = await createImageBitmap(blob);
    const target = fitWithin(probe.width, probe.height, THUMBNAIL_MAX_EDGE);
    probe.close?.();
    const bitmap = await createImageBitmap(blob, {
      resizeHeight: target.height,
      resizeQuality: "high",
      resizeWidth: target.width,
    });
    try {
      return await canvasToBlob(
        drawToCanvas(bitmap, bitmap.width, bitmap.height),
        "image/png",
      );
    } finally {
      bitmap.close?.();
    }
  }

  // SVG path: render through <img> straight at thumbnail size, so no full-size raster
  // is ever allocated.
  const sized =
    mimeType === "image/svg+xml"
      ? svgWithExplicitSize(new TextDecoder("utf-8").decode(bytes))
      : null;
  const loaded = await loadImageElement(
    sized ? new Blob([sized.source], { type: "image/svg+xml" }) : blob,
    sized ?? undefined,
  );
  const target = fitWithin(loaded.width, loaded.height, THUMBNAIL_MAX_EDGE);
  return canvasToBlob(
    drawToCanvas(loaded.image, target.width, target.height),
    "image/png",
  );
}

/**
 * Generate a thumbnail for an image that is already in the backend's blob store.
 *
 * The paste path produces thumbnails as a side effect of reading the clipboard, but the
 * other ways an image arrives — the file picker, a message's history — hand the backend
 * bytes the webview never saw. Without this those attachments have no thumbnail, and the
 * strip is left rendering full-resolution originals.
 *
 * Bytes come over the resource protocol rather than the message channel, so nothing large
 * crosses a process boundary. The compressed file does land on the JS heap for the length
 * of this call, but only the compressed form: `makeThumbnail` resizes inside the decoder,
 * so the full-size bitmap is never allocated.
 */
export async function makeThumbnailFromUrl(
  url: string,
  mimeType: string,
): Promise<Blob> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`could not read image (${response.status})`);
  }
  return makeThumbnail(await response.arrayBuffer(), mimeType);
}

/**
 * Load an image as a same-origin blob URL, which is the only way SVG displays.
 *
 * VS Code sets a webview resource's `Content-Type` from the file extension alone
 * (`platform/webview/common/mimeTypes.ts`), and content-addressed blobs are named by
 * hash — no extension, so the type comes back as unknown. A raster survives that because
 * browsers sniff the bytes of an `<img>`; SVG does not, because SVG is only treated as an
 * image when the type says so. Re-wrapping the bytes in a typed blob supplies the type
 * the protocol cannot, and keeps SVG a vector rather than showing a rasterised stand-in.
 *
 * The caller owns the returned URL and must revoke it.
 */
export async function typedBlobUrl(url: string, mimeType: string): Promise<string> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`could not read image (${response.status})`);
  }
  return URL.createObjectURL(new Blob([await response.arrayBuffer()], { type: mimeType }));
}

/** True when an SVG is mostly an embedded raster, making its source useless as text. */
export function svgSourceIsUninformative(source: string): boolean {
  return /<image\b[^>]*(?:xlink:)?href\s*=\s*["']\s*data:image\//i.test(source);
}

function describeError(error: unknown): string {
  if (error instanceof Error) {
    return error.name === "SecurityError" ? "canvas was tainted" : error.message;
  }
  return String(error);
}

/**
 * Turn raw pasted or dropped bytes into everything the backend needs, in one pass.
 *
 * Never throws: each derived artefact is optional and its absence is recorded as a
 * warning. The original bytes always survive, so the user always sees their image.
 */
export async function prepareAttachment(
  raw: RawAttachment,
): Promise<PreparedAttachment> {
  const warnings: string[] = [];
  const prepared: PreparedAttachment = {
    dataBase64: toBase64(raw.bytes),
    filename: raw.filename,
    mimeType: raw.mimeType,
    sourcePath: raw.sourcePath ?? null,
    warnings,
  };

  if (!raw.mimeType.startsWith("image/")) {
    return prepared;
  }

  try {
    prepared.thumbBase64 = await blobToBase64(await makeThumbnail(raw.bytes, raw.mimeType));
  } catch (error) {
    // Losing the thumbnail costs memory, not correctness: rendering falls back to the
    // full image. Worth a note, not worth rejecting the attachment.
    warnings.push(t("image.noThumbnail", { detail: describeError(error) }));
  }

  if (raw.mimeType === "image/svg+xml") {
    try {
      prepared.providerBase64 = await blobToBase64(await rasterizeSvgToPng(raw.bytes));
      prepared.providerMimeType = "image/png";
    } catch {
      // Degradation step 2: hand the model the source instead of a raster.
      const source = new TextDecoder("utf-8").decode(raw.bytes);
      if (raw.bytes.byteLength > SVG_SOURCE_MAX_BYTES) {
        warnings.push(
          t("image.svgTooLarge"),
        );
      } else if (svgSourceIsUninformative(source)) {
        warnings.push(
          t("image.svgBitmapOnly"),
        );
      } else {
        prepared.providerText = source;
        warnings.push(t("image.svgAsSource"));
      }
    }
  }

  return prepared;
}
