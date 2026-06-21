import type { WebviewPendingAttachment } from "../types";

export function AttachmentChips({
  attachments,
  onRemove,
}: {
  attachments: WebviewPendingAttachment[];
  onRemove(attachmentId: string): void;
}) {
  if (!attachments.length) {
    return null;
  }

  return (
    <section className="tc-attachment-chips" aria-label="Pending attachments">
      {attachments.map((attachment) => (
        <button
          aria-label={`Remove attachment ${attachment.label}`}
          className="tc-chip tc-chip--attachment"
          data-testid="attachment-chip"
          key={attachment.id}
          onClick={() => onRemove(attachment.id)}
          type="button"
        >
          <span>{attachment.label}</span>
          <span className="tc-chip__close">x</span>
        </button>
      ))}
    </section>
  );
}
