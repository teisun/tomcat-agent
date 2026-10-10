/**
 * Attachment ingest — the single point where image bytes cross into the backend.
 *
 * ```
 *   paste / drop / file pick
 *     │
 *     │  webview: decode, downsample to a thumbnail, rasterise SVG
 *     ▼
 *   ingestAttachment()  ── the ONLY protocol message that carries bytes ──►  Rust
 *     │                                                                      │
 *     │  ◄── { blobSha, providerSha, hasThumb, bytes } ───────────────────────┘
 *     ▼
 *   draft holds references from here on; sending carries only hashes
 * ```
 *
 * Everything downstream — the draft file, state snapshots, the prompt itself — speaks
 * in sha256 handles. That is what keeps a keystroke from costing megabytes.
 *
 * The backend re-validates whatever arrives here. The webview's own checks exist to
 * give immediate feedback, and are never trusted: a compromised webview can only get
 * bytes rejected, not smuggled through.
 */
import type { TomcatMessenger } from "../serveClient/TomcatMessenger";
import type {
  IngestAttachmentInput,
  IngestAttachmentResponse,
  ServeAttachmentKind,
} from "../serveClient/wire";

import type { DraftAttachmentRef } from "./composerDraft";
import { t } from "./i18n";

/** Everything the webview managed to derive from one pasted image. */
export interface AttachmentUpload {
  dataBase64: string;
  filename?: string | null;
  kind: ServeAttachmentKind;
  mimeType: string;
  providerBase64?: string;
  providerMimeType?: string;
  /** SVG source to send as text when rasterisation was not possible. */
  providerText?: string;
  /** Original disk path, when this came from drag/drop or the file picker. */
  sourcePath?: string | null;
  thumbBase64?: string;
}

export type IngestOutcome =
  | { error: string; ok: false }
  | { ok: true; reference: DraftAttachmentRef };

/**
 * Hand one attachment's bytes to the backend and get a reference back.
 *
 * Rejection is a value, not an exception: a batch paste of eleven images where one is
 * oversized should attach ten and explain the one, rather than failing as a unit.
 */
export async function ingestAttachment(
  messenger: TomcatMessenger,
  sessionId: string,
  id: string,
  upload: AttachmentUpload,
): Promise<IngestOutcome> {
  const attachment: IngestAttachmentInput = {
    dataBase64: upload.dataBase64,
    filename: upload.filename ?? null,
    kind: upload.kind,
    mimeType: upload.mimeType,
    providerBase64: upload.providerBase64 ?? null,
    providerMimeType: upload.providerMimeType ?? null,
    thumbBase64: upload.thumbBase64 ?? null,
  };

  let response;
  try {
    response = await messenger.request({
      attachment,
      sessionId,
      type: "ingest_attachment",
    });
  } catch (error) {
    return {
      error: error instanceof Error ? error.message : String(error),
      ok: false,
    };
  }

  if (!response.success || !response.payload) {
    return { error: response.error ?? t("attachment.ingestFailed"), ok: false };
  }

  const payload = parseIngestResponse(response.payload);
  if (!payload) {
    // Checked rather than cast. Every reference downstream — the draft file, the state
    // snapshot, the URL the webview fetches — is built from `blobSha`, so letting an
    // unverified one through produces an attachment that exists in the UI and resolves
    // to nothing, with the cause several layers away from the symptom.
    return { error: t("attachment.ingestInvalid"), ok: false };
  }

  return {
    ok: true,
    reference: {
      blobSha: payload.blobSha,
      bytes: payload.bytes,
      filename: payload.filename,
      // A thumbnail is addressed by the hash of the image it came from, so there is
      // nothing to remember beyond whether one exists.
      hasThumb: payload.hasThumb,
      id,
      kind: upload.kind,
      mimeType: payload.mimeType,
      providerSha: payload.providerSha ?? null,
      providerText: upload.providerText ?? null,
      sourcePath: upload.sourcePath ?? null,
    },
  };
}

function parseIngestResponse(value: unknown): IngestAttachmentResponse | null {
  if (typeof value !== "object" || value === null) return null;
  const payload = value as Record<string, unknown>;
  if (
    typeof payload.blobSha !== "string" ||
    !/^[0-9a-f]{64}$/.test(payload.blobSha) ||
    typeof payload.filename !== "string" ||
    typeof payload.mimeType !== "string"
  ) {
    return null;
  }
  return {
    blobSha: payload.blobSha,
    bytes: typeof payload.bytes === "number" ? payload.bytes : 0,
    filename: payload.filename,
    hasThumb: payload.hasThumb === true,
    mimeType: payload.mimeType,
    providerSha: typeof payload.providerSha === "string" ? payload.providerSha : null,
  };
}

/**
 * Store a thumbnail for bytes that are already in the backend.
 *
 * Used for history images, whose thumbnails cannot be generated at paste time because
 * they were pasted in some earlier session — possibly by an earlier build that had no
 * thumbnails at all. Failure is silent by design: a missing thumbnail costs memory,
 * and complaining about it would be noise the user cannot act on.
 */
export async function cacheAttachmentThumbnail(
  messenger: TomcatMessenger,
  sessionId: string,
  sourceSha: string,
  thumbBase64: string,
): Promise<boolean> {
  try {
    const response = await messenger.request({
      sessionId,
      thumbnail: { sourceSha, thumbBase64 },
      type: "cache_attachment_thumbnail",
    });
    return response.success;
  } catch {
    return false;
  }
}
