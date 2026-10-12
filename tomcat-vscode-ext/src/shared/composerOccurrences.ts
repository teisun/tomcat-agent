import type { WebviewMessageSegment } from "../ui/webview/protocol";

/** Identity of one user insertion, deliberately absent from the Serve wire. */
export function withOccurrence<T extends Exclude<WebviewMessageSegment, { type: "text" }>>(segment: T): T {
  return segment.occurrenceId ? segment : { ...segment, occurrenceId: crypto.randomUUID() };
}

/** Refill/HTML copy is a new insertion; undo/redo and draft hydration are not. */
export function freshOccurrences(segments: WebviewMessageSegment[]): WebviewMessageSegment[] {
  return segments.map((segment) => segment.type === "text" ? { ...segment } : withOccurrence({ ...segment, occurrenceId: undefined }));
}
