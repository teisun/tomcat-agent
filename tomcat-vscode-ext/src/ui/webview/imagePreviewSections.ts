import type { PreviewSection } from "../../shared/imagePreviewProtocol";
import type { WebviewAttachmentView, WebviewSessionSnapshot } from "./protocol";

/** Reference-only gallery. Never substitute a full image for a missing thumbnail. */
export function imagePreviewSections(session: WebviewSessionSnapshot | undefined): PreviewSection[] {
  if (!session) return [];
  const usable = (attachment: WebviewAttachmentView) =>
    attachment.kind === "image" && !attachment.unavailable && Boolean(attachment.fullUri);
  const toPicture = (attachment: WebviewAttachmentView) => ({
    filename: attachment.filename, fullUri: attachment.fullUri ?? "", id: attachment.id,
    mimeType: attachment.mimeType, thumbUri: attachment.thumbUri ?? null,
  });
  const pending = session.pendingAttachments.filter(usable).map(toPicture);
  const pendingIds = new Set(pending.map(picture => picture.id));
  const history = session.timeline.flatMap((item, index) => {
    if (item.type !== "tool" && (item.type !== "message" || item.kind !== "user")) return [];
    const pictures = (item.attachments ?? []).filter(a => usable(a) && !pendingIds.has(a.id)).map(toPicture);
    const label = item.type === "tool" ? `Tool images · ${item.summaryTitle || item.toolName}` : `Sent images ${index + 1}`;
    return pictures.length ? [{ label, pictures }] : [];
  });
  return [...(pending.length ? [{ label: "Pending", pictures: pending }] : []), ...history];
}
