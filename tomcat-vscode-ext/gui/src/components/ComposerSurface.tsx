import { forwardRef } from "react";
import { AttachmentStrip } from "./AttachmentStrip";
import { Composer, type ComposerHandle, type ComposerProps } from "./Composer";
import type { WebviewPendingAttachment } from "../types";

export interface ComposerSurfaceProps extends ComposerProps {
  attachments: WebviewPendingAttachment[];
  onOpenAttachment(attachment: WebviewPendingAttachment): void;
  onRemoveAttachment(id: string): void;
  feedback?: { hasErrors: boolean; message: string } | null;
}

/** Both compose locations use the same attachment/editor/settings surface. */
export const ComposerSurface = forwardRef<ComposerHandle, ComposerSurfaceProps>(function ComposerSurface({
  attachments, onOpenAttachment, onRemoveAttachment, feedback, ...composer
}, ref) {
  return <>
    <AttachmentStrip attachments={attachments} onOpen={onOpenAttachment} onRemove={composer.canPrompt ? onRemoveAttachment : undefined} readonly={!composer.canPrompt} />
    {feedback ? <div className={feedback.hasErrors ? "tc-attachment-feedback tc-attachment-feedback--error" : "tc-visually-hidden"}
      data-testid={feedback.hasErrors ? "attachment-feedback-error" : "attachment-feedback-announcement"} role="status">{feedback.message}</div> : null}
    <Composer {...composer} ref={ref} />
  </>;
});
