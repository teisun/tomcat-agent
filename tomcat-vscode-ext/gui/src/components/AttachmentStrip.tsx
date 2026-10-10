import { useT } from "../i18n/LocaleProvider";
import type { KeyboardEvent } from "react";

import type { WebviewPendingAttachment } from "../types";

/**
 * AttachmentStrip — horizontal strip of image thumbnails and file chips.
 *
 * Used in both the Composer (pending) and history messages (read-only).
 * Images: 48px square thumbnails, object-fit: cover, with optional delete button.
 * PDFs: 48px square chips with a PDF icon and tooltip.
 * Empty list: returns null (zero height).
 *
 * The strip only ever loads `thumbUri`. A 48px square has no use for a 4000x3000
 * decode, and asking for one anyway was how eleven pasted photos turned into hundreds
 * of megabytes of bitmap. The full-resolution URL exists, but only the preview panel is
 * allowed to touch it.
 */
export function AttachmentStrip({
  attachments,
  onRemove,
  onOpen,
  readonly = false,
}: {
  attachments: WebviewPendingAttachment[];
  onRemove?(attachmentId: string): void;
  onOpen?(attachment: WebviewPendingAttachment): void;
  readonly?: boolean;
}) {
  const t = useT();
  if (!attachments.length) {
    return null;
  }

  return (
    <section
      className={`tc-attachment-strip${readonly ? " tc-attachment-strip--history" : ""}`}
      aria-label={t(readonly ? "attachment.history" : "attachment.pending")}
      data-attachment-source={readonly ? "history" : "draft"}
      role="list"
    >
      {attachments.map((attachment) => {
        const isImage = attachment.kind === "image";
        const removeButton =
          !readonly && onRemove ? (
            <button
              aria-label={t("attachment.remove", { name: attachment.label })}
              className="tc-attachment-strip__close"
              data-testid="attachment-remove"
              onClick={(e) => {
                e.stopPropagation();
                onRemove(attachment.id);
              }}
              title={t("attachment.removeAction")}
              type="button"
            >
              ×
            </button>
          ) : null;
        // Delete on a focused thumbnail, so the strip is usable without a mouse. The
        // click target opens the preview, which leaves removal with no keyboard path
        // otherwise.
        const removeOnDeleteKey = (event: KeyboardEvent) => {
          if (readonly || !onRemove) return;
          if (event.key !== "Delete" && event.key !== "Backspace") return;
          event.preventDefault();
          onRemove(attachment.id);
        };

        // The bytes this attachment names are gone from the backend. Say so, keep the
        // remove button, and leave the rest of the draft alone.
        if (attachment.unavailable) {
          // Spelled out for screen readers and on hover, because the strip has room for
          // a filename and not for a sentence. A blank square or a broken-image glyph
          // would leave the user guessing.
          const explanation = t("attachment.unavailable", { name: attachment.label });
          return (
            <div
              key={attachment.id}
              className="tc-attachment-strip__item"
              role="listitem"
            >
              <span
                aria-label={explanation}
                className="tc-chip tc-chip--attachment tc-chip--unavailable"
                data-testid="attachment-unavailable"
                title={explanation}
              >
                <span aria-hidden="true" className="tc-chip__icon">⚠</span>
                <span>{attachment.label}</span>
              </span>
              {removeButton}
            </div>
          );
        }

        // Thumbnails are generated in the webview and stored by the backend, so for a
        // moment after a paste — and while a history image's thumbnail is being cached —
        // there is nothing to show yet. A placeholder of exactly the final size keeps the
        // composer from jumping as the images land.
        if (isImage && !attachment.thumbUri) {
          return (
            <div
              key={attachment.id}
              className="tc-attachment-strip__item"
              role="listitem"
            >
              <span
                aria-label={t("attachment.loading", { name: attachment.label })}
                className="tc-attachment-strip__skeleton"
                data-testid="attachment-skeleton"
                title={attachment.label}
              />
              {removeButton}
            </div>
          );
        }

        if (isImage && attachment.thumbUri) {
          return (
            <div
              key={attachment.id}
              className="tc-attachment-strip__item"
              role="listitem"
            >
              <button
                aria-label={t("attachment.open", { name: attachment.label })}
                className="tc-attachment-strip__thumb"
                data-testid={readonly ? "history-attachment-thumb" : "attachment-thumb"}
                onClick={() => onOpen?.(attachment)}
                onKeyDown={removeOnDeleteKey}
                title={attachment.label}
                type="button"
              >
                <img
                  alt={attachment.label}
                  className="tc-attachment-strip__img"
                  data-attachment-resolution={attachment.hasThumb ? "thumb" : "full"}
                  decoding="async"
                  loading="lazy"
                  src={attachment.thumbUri}
                />
              </button>
              {removeButton}
            </div>
          );
        }

        // Non-image attachments (PDFs, etc)
        const tooltip = attachment.path ?? describeAttachment(attachment);
        const fileLabel = fileChipLabel(attachment);
        const chipContents = (
          <>
            <span
              aria-hidden="true"
              className={`tc-attachment-strip__file-icon codicon ${fileLabel === "PDF" ? "codicon-file-pdf" : "codicon-file"}`}
            />
            <span aria-hidden="true" className="tc-attachment-strip__file-label">{fileLabel}</span>
          </>
        );
        const isClickable = Boolean(attachment.path && onOpen);
        return (
          <div
            key={attachment.id}
            className="tc-attachment-strip__item"
            role="listitem"
          >
            {isClickable ? (
              <button
                aria-label={t("attachment.open", { name: attachment.label })}
                className="tc-attachment-strip__thumb tc-attachment-strip__file-chip"
                data-testid={readonly ? "history-attachment-chip" : "attachment-chip"}
                onClick={() => onOpen?.(attachment)}
                onKeyDown={removeOnDeleteKey}
                title={tooltip}
                type="button"
              >
                {chipContents}
              </button>
            ) : (
              <span
                aria-label={attachment.label}
                className="tc-attachment-strip__thumb tc-attachment-strip__file-chip"
                data-testid={readonly ? "history-attachment-chip" : "attachment-chip"}
                title={tooltip}
              >
                {chipContents}
              </span>
            )}
            {removeButton}
          </div>
        );
      })}
    </section>
  );
}

function describeAttachment(attachment: WebviewPendingAttachment): string {
  const size = formatAttachmentSize(attachment.bytes);
  return size ? `${attachment.label} · ${size}` : attachment.label;
}

function fileChipLabel(attachment: WebviewPendingAttachment): string {
  if (attachment.mimeType === "application/pdf") {
    return "PDF";
  }
  const name = (attachment.filename ?? attachment.label).trim();
  const match = /\.([A-Za-z0-9]+)$/.exec(name);
  return match ? match[1]!.toUpperCase() : "FILE";
}

function formatAttachmentSize(bytes: number | undefined): string | null {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) {
    return null;
  }
  if (bytes >= 1024 * 1024) {
    return formatSizeUnit(bytes / (1024 * 1024), "MB");
  }
  if (bytes >= 1024) {
    return formatSizeUnit(bytes / 1024, "KB");
  }
  return `${bytes} B`;
}

function formatSizeUnit(value: number, unit: "KB" | "MB"): string {
  const rounded = value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${Number.isInteger(rounded) ? rounded : rounded.toFixed(1)} ${unit}`;
}
