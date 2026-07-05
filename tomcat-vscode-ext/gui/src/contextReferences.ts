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

export function referenceIdentity(reference: WebviewReference): string {
  return [
    reference.kind,
    reference.path,
    reference.lineStart ?? "",
    reference.lineEnd ?? "",
  ].join("::");
}
