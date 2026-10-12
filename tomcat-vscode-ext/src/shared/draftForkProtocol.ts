import type { WebviewMessageSegment } from "../ui/webview/protocol";

/**
 * Click-time draft fork contract.
 *
 * ┌──────────────┐ cutoff ┌────────────────┐ persist source ┌─────────────────┐
 * │ live Composer├────────►wait pre-click  ├────────────────►detached target  │
 * └──────────────┘         │work only       │ retain leases  │install-if-empty │
 *                          └────────────────┘                 └────────┬────────┘
 *                                                                    │ commit
 *                                                                    ▼
 *                                                               select target
 *
 * The source is immutable after capture. Target creation is detached, so neither the
 * serve registry nor sessions.json.current changes before the final select. Any failure
 * before select removes only target artifacts/leases and leaves the source active.
 * A host crash may leave a detached target, but the durable source draft remains recoverable.
 */
export const DRAFT_FORK_LIMITS = Object.freeze({
  cwdBytes: 4 * 1024,
  operationIdBytes: 128,
  payloadBytes: 2 * 1024 * 1024,
  segmentCount: 4_096,
  sessionIdBytes: 128,
  textBytes: 1024 * 1024,
});

export interface DraftForkCapture {
  cwd: string | null;
  operationId: string;
  segments: WebviewMessageSegment[];
  sourceSessionId: string;
  text: string;
}

export interface DraftForkResult {
  error?: string;
  operationId: string;
  sourceSessionId: string;
  success: boolean;
  targetSessionId?: string;
}

export type DraftForkCommitStep =
  | "persist_source"
  | "create_detached"
  | "retain_target_leases"
  | "install_target_draft"
  | "select_target";

function utf8Bytes(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isReferenceSegment(value: unknown): value is Extract<WebviewMessageSegment, { type: "reference" }> {
  if (!isRecord(value) || value.type !== "reference") return false;
  return (
    (value.kind === "file" || value.kind === "selection")
    && typeof value.label === "string"
    && typeof value.path === "string"
    && (value.lineStart === undefined || value.lineStart === null || Number.isInteger(value.lineStart))
    && (value.lineEnd === undefined || value.lineEnd === null || Number.isInteger(value.lineEnd))
    && (value.text === undefined || value.text === null || typeof value.text === "string")
  );
}

function isSegment(value: unknown): value is WebviewMessageSegment {
  return isRecord(value) && (
    (value.type === "text" && typeof value.text === "string")
    || (value.type === "instruction" && (value.kind === "command" || value.kind === "skill") && typeof value.resourceId === "string" && typeof value.label === "string")
    || isReferenceSegment(value)
  );
}

export function parseDraftForkCapture(value: unknown): DraftForkCapture | null {
  if (!isRecord(value)) return null;
  const { cwd, operationId, segments, sourceSessionId, text } = value;
  if (
    typeof operationId !== "string"
    || operationId.length === 0
    || utf8Bytes(operationId) > DRAFT_FORK_LIMITS.operationIdBytes
    || typeof sourceSessionId !== "string"
    || sourceSessionId.length === 0
    || utf8Bytes(sourceSessionId) > DRAFT_FORK_LIMITS.sessionIdBytes
    || typeof text !== "string"
    || utf8Bytes(text) > DRAFT_FORK_LIMITS.textBytes
    || (cwd !== null && cwd !== undefined && typeof cwd !== "string")
    || (typeof cwd === "string" && utf8Bytes(cwd) > DRAFT_FORK_LIMITS.cwdBytes)
    || !Array.isArray(segments)
    || segments.length > DRAFT_FORK_LIMITS.segmentCount
    || !segments.every(isSegment)
  ) {
    return null;
  }
  if (utf8Bytes(JSON.stringify(value)) > DRAFT_FORK_LIMITS.payloadBytes) return null;
  return {
    cwd: typeof cwd === "string" ? cwd : null,
    operationId,
    // The operation owns an immutable copy; later Composer edits cannot mutate the cutoff.
    segments: structuredClone(segments),
    sourceSessionId,
    text,
  };
}
