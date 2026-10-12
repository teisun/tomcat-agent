import type { WebviewAttachmentView, WebviewSessionSnapshot } from "../types";

export type ThumbnailTarget = { blobSha: string; fullUri: string; mimeType: string };

/** Pick exactly one resource; the caller owns the serial decode and attempted set. */
export function nextThumbnailTarget(
  session: Pick<WebviewSessionSnapshot, "pendingAttachments" | "timeline">,
  attempted: ReadonlySet<string>,
): ThumbnailTarget | null {
  const candidate = (attachment: WebviewAttachmentView): ThumbnailTarget | null =>
    attachment.kind === "image" && !attachment.unavailable && !attachment.hasThumb &&
    attachment.fullUri && attachment.mimeType.startsWith("image/") && !attempted.has(attachment.blobSha)
      ? { blobSha: attachment.blobSha, fullUri: attachment.fullUri, mimeType: attachment.mimeType }
      : null;
  for (const attachment of session.pendingAttachments) {
    const target = candidate(attachment); if (target) return target;
  }
  for (const item of session.timeline) {
    if (item.type !== "message" && item.type !== "tool") continue;
    for (const attachment of item.attachments ?? []) {
      const target = candidate(attachment); if (target) return target;
    }
  }
  return null;
}
