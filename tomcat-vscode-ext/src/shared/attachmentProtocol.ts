/**
 * Shared protocol types for attachments between the chat webview and the host.
 *
 * Data flow:
 *   webview paste/drop/pick → downsample + rasterise in the webview (images only)
 *     → attachFiles (the only message that ever carries attachment bytes)
 *     → host decodes and forwards to ingest_attachment
 *     → host keeps the returned hash in the draft store, and nothing else
 */

import { t } from "./i18n";

export const IMAGE_MAX_BYTES = 4_718_592;
export const PDF_MAX_BYTES = 25 * 1024 * 1024;
export const ALLOWED_IMAGE_MIME_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/svg+xml",
] as const;
export const ALLOWED_FILE_MIME_TYPES = ["application/pdf"] as const;
export const ALLOWED_ATTACHMENT_MIME_TYPES = [
  ...ALLOWED_IMAGE_MIME_TYPES,
  ...ALLOWED_FILE_MIME_TYPES,
] as const;

export type AllowedImageMimeType = (typeof ALLOWED_IMAGE_MIME_TYPES)[number];
export type AllowedFileMimeType = (typeof ALLOWED_FILE_MIME_TYPES)[number];
export type AllowedAttachmentMimeType = (typeof ALLOWED_ATTACHMENT_MIME_TYPES)[number];

export function isSupportedAttachmentMime(mimeType: string): mimeType is AllowedAttachmentMimeType {
  return (ALLOWED_ATTACHMENT_MIME_TYPES as readonly string[]).includes(mimeType);
}

export function decodeBase64Strict(dataBase64: string): Buffer | null {
  if (dataBase64.length === 0 || dataBase64.length % 4 !== 0) {
    return null;
  }
  const firstPadding = dataBase64.indexOf("=");
  const contentEnd = firstPadding === -1 ? dataBase64.length : firstPadding;
  const paddingLength = dataBase64.length - contentEnd;
  if (paddingLength > 2) return null;
  for (let index = 0; index < contentEnd; index += 1) {
    const code = dataBase64.charCodeAt(index);
    const valid =
      (code >= 65 && code <= 90) ||
      (code >= 97 && code <= 122) ||
      (code >= 48 && code <= 57) ||
      code === 43 ||
      code === 47;
    if (!valid) return null;
  }
  for (let index = contentEnd; index < dataBase64.length; index += 1) {
    if (dataBase64[index] !== "=") return null;
  }
  const bytes = Buffer.from(dataBase64, "base64");
  return bytes.toString("base64") === dataBase64 ? bytes : null;
}

export function safeAttachmentFilename(
  filename: string | null | undefined,
  fallback: string,
): string {
  const basename = (filename ?? "").split(/[\\/]/).pop()?.trim() ?? "";
  const normalized = basename
    .replace(/[\u0000-\u001f\u007f]/g, "")
    .replace(/^\.+/, "")
    .slice(0, 180);
  return normalized || fallback;
}

/**
 * Reject what the user should hear about immediately: an unknown type, a corrupt
 * payload, an oversized paste.
 *
 * Deliberately *not* a security boundary, and deliberately not inspecting SVG markup.
 * SVG is only ever loaded through `<img>`, which the HTML spec puts in secure static
 * mode: scripts do not run and external references are not fetched. The blacklist this
 * replaced rejected `style=` and `url(`, which meant nearly every icon exported from
 * Figma or Illustrator was refused as "unsafe" while a namespace-aliased `x:href`
 * walked straight through. The real check on the bytes is the backend's magic-byte
 * sniff, which cannot be talked out of its answer.
 */
export function validateAttachmentCandidate(candidate: AttachmentCandidate):
  | {
      bytes: Buffer;
      filename: string;
      kind: "file" | "image";
      mimeType: AllowedAttachmentMimeType;
      ok: true;
    }
  | {
      error: string;
      ok: false;
    } {
  if (candidate.mimeType === "application/pdf") {
    const bytes = decodeBase64Strict(candidate.dataBase64);
    if (!bytes) return { error: t("attachment.invalidBase64"), ok: false };
    if (bytes.length > PDF_MAX_BYTES) {
      return { error: t("attachment.pdfTooLarge"), ok: false };
    }
    return {
      bytes,
      filename: safeAttachmentFilename(candidate.filename, "attachment.pdf"),
      kind: "file",
      mimeType: candidate.mimeType,
      ok: true,
    };
  }

  if (!(ALLOWED_IMAGE_MIME_TYPES as readonly string[]).includes(candidate.mimeType)) {
    return { error: t("attachment.unsupportedType", { mime: candidate.mimeType }), ok: false };
  }
  const bytes = decodeBase64Strict(candidate.dataBase64);
  if (!bytes) return { error: t("attachment.invalidBase64"), ok: false };
  if (bytes.length > IMAGE_MAX_BYTES) {
    return { error: t("attachment.imageTooLarge"), ok: false };
  }
  const extension =
    candidate.mimeType === "image/jpeg"
      ? "jpg"
      : candidate.mimeType === "image/svg+xml"
        ? "svg"
        : candidate.mimeType.split("/")[1];
  return {
    bytes,
    filename: safeAttachmentFilename(
      candidate.filename,
      `pasted-image.${extension}`,
    ),
    kind: "image",
    mimeType: candidate.mimeType as AllowedImageMimeType,
    ok: true,
  };
}

/** One attachment candidate from a paste/drop/pick event. */
export interface AttachmentCandidate {
  /** Raw base64-encoded attachment bytes (no data: URL prefix). */
  dataBase64: string;
  /** Optional filename from paste/drop (may be falsy for clipboard screenshots). */
  filename?: string | null;
  /**
   * Original local path when the host surface exposed one.
   *
   * Display-only metadata: used for hover/open affordances, never as the backend's
   * authority for what bytes an attachment means.
   */
  sourcePath?: string | null;
  /** W3C MIME, e.g. image/png or application/pdf. */
  mimeType: string;
  /** PNG rendering for formats providers reject. Base64. */
  providerBase64?: string;
  providerMimeType?: string;
  /** SVG source to send as text when rasterisation was not possible. */
  providerText?: string;
  /** Downsampled preview, PNG, base64. Absent when generation failed. */
  thumbBase64?: string;
  /** Non-fatal notes from the image pipeline, surfaced to the user. */
  warnings?: string[];
}

/** Result item after validation — accepted or rejected. */
export interface AttachmentResultItem {
  /** Stable attachment id, set only for accepted items. */
  id?: string;
  /** Filename as stored (defaulted if paste had none). */
  filename?: string;
  /** MIME type as stored. */
  mimeType?: string;
  /** Error message for rejected items. Only present on failure. */
  error?: string;
}

/** Webview → Host: attach one or more attachments from paste/drop/pick. */
export interface AttachFilesIntent {
  messageId: string;
  type: "attachFiles";
  data: {
    files: AttachmentCandidate[];
    sessionId: string;
  };
}

/** Host → Webview: result of attachment validation and save. */
export interface AttachFilesResult {
  type: "attachFilesResult";
  /** Per-item results in the same order as input files[]. */
  items: AttachmentResultItem[];
}

/** Host → Webview: validation error for paste/drop/pick. */
export interface AttachmentFeedback {
  type: "attachmentFeedback";
  data: {
    /** General feedback message (e.g. "3 attachments added, 1 skipped"). */
    message: string;
    /** true if any attachment was rejected */
    hasErrors: boolean;
  };
}
