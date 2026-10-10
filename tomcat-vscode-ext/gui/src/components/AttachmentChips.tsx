import { useT } from "../i18n/LocaleProvider";
import type { WebviewPendingAttachment } from "../types";

export function AttachmentChips({
  attachments,
  onRemove,
}: {
  attachments: WebviewPendingAttachment[];
  onRemove(attachmentId: string): void;
}) {
  const t = useT();
  if (!attachments.length) {
    return null;
  }

  return (
    <section className="tc-attachment-chips" aria-label={t("attachment.pending")}>
      {attachments.map((attachment) => (
        <button
          aria-label={t("attachment.removeNamed", { name: attachment.label })}
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
