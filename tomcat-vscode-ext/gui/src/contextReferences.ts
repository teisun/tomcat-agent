import type { WebviewReference } from "./types";

function isNullableNumber(value: unknown): value is number | null | undefined {
  return value === undefined || value === null || typeof value === "number";
}

export function buildReferenceTitle(reference: WebviewReference): string {
  if (reference.kind !== "selection") {
    return reference.path;
  }
  if (typeof reference.lineStart === "number" && typeof reference.lineEnd === "number") {
    return reference.lineStart === reference.lineEnd
      ? `${reference.path}:${reference.lineStart}`
      : `${reference.path}:${reference.lineStart}-${reference.lineEnd}`;
  }
  if (typeof reference.lineStart === "number") {
    return `${reference.path}:${reference.lineStart}`;
  }
  return reference.path;
}

export function isWebviewReference(value: unknown): value is WebviewReference {
  if (!value || typeof value !== "object") {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    candidate.type === "reference" &&
    (candidate.kind === "selection" || candidate.kind === "file") &&
    typeof candidate.label === "string" &&
    typeof candidate.path === "string" &&
    isNullableNumber(candidate.lineStart) &&
    isNullableNumber(candidate.lineEnd) &&
    (candidate.text === undefined || candidate.text === null || typeof candidate.text === "string")
  );
}

/**
 * Bounded, stable string hash (FNV-1a, 32-bit) used only to keep dedupe keys
 * short. Not cryptographic.
 */
function hashText(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
}

export function referenceIdentity(reference: WebviewReference): string {
  const parts: Array<number | string> = [
    reference.kind,
    reference.path,
    reference.lineStart ?? "",
    reference.lineEnd ?? "",
  ];
  // Line-less selections (e.g. plan-preview text whose source line could not be
  // resolved) would otherwise all collapse to `selection::<path>::::` and dedupe
  // each other away, so only the first snippet per file could ever be inserted.
  // Discriminate them by a hash of the selected text.
  if (
    reference.kind === "selection" &&
    reference.lineStart == null &&
    reference.lineEnd == null
  ) {
    parts.push(hashText(reference.text ?? ""));
  }
  return parts.join("::");
}
