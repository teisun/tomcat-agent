import { memo, useEffect, useState } from "react";
import { useT } from "../i18n/LocaleProvider";

import { AttachmentStrip } from "./AttachmentStrip";
import { ReferenceChip } from "./ReferenceChip";
import { InvocationChip } from "./InvocationChip";
import { ChatMarkdown } from "./markdown/ChatMarkdown";
import type {
  WebviewMessageBlock,
  WebviewMediaRoot,
  WebviewMessageSegment,
  WebviewPendingAttachment,
  PathResolution,
} from "../types";

const NOOP_OPEN_FILE = () => undefined;

type MessageBubbleProps = {
  onEdit?(item: WebviewMessageBlock): void;
  hasFileWrites?: boolean;
  item: WebviewMessageBlock;
  mediaRoots?: WebviewMediaRoot[];
  onOpenFile?: (path: string, line?: number) => void;
  onOpenLink?: (href: string) => void;
  onOpenImagePreview?: (imageId: string) => void;
  onRecover?: (messageId: string, action: "resume" | "retry") => void;
  onRetry?: (messageId: string) => void;
  resolvePaths?: (paths: string[]) => Promise<PathResolution[]>;
  recoveryDisabled?: boolean;
  onZoomImage?: (image: { alt: string; src: string }) => void;
};

function MessageBubbleComponent({
  onEdit,
  hasFileWrites,
  item,
  mediaRoots,
  onOpenFile,
  onOpenLink,
  onOpenImagePreview,
  onRecover,
  onRetry,
  resolvePaths,
  recoveryDisabled = false,
  onZoomImage,
}: MessageBubbleProps) {
  const t = useT();
  const [detailsExpanded, setDetailsExpanded] = useState(false);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const isFailedUserMessage = item.kind === "user" && item.deliveryState === "failed";
  const isPendingUserMessage = item.kind === "user" && item.deliveryState === "pending";
  const isAbandonedUserMessage = item.kind === "user" && item.abandoned === true;
  const showRetry = isFailedUserMessage && item.retryable === true && typeof onRetry === "function";
  const recoveryAction =
    item.kind === "error" && item.recoveryAction && typeof onRecover === "function"
      ? item.recoveryAction
      : null;
  const showHeader =
    item.kind !== "user" && item.kind !== "assistant" && recoveryAction === null;
  const rawErrorDetail =
    item.kind === "error" && typeof item.detailText === "string" && item.detailText.trim().length > 0
      ? item.detailText
      : null;
  const canToggleRawError = rawErrorDetail !== null && rawErrorDetail.trim() !== item.text.trim();
  const segments: WebviewMessageSegment[] =
    item.segments?.length ? item.segments : [{ text: item.text, type: "text" }];
  const historyAttachments: WebviewPendingAttachment[] = (item.attachments ?? []).map(
    (attachment) => ({
      ...attachment,
      label: attachment.filename,
      path: null,
    }),
  );

  useEffect(() => {
    setDetailsExpanded(false);
    setCopyState("idle");
  }, [item.id, item.detailText]);

  async function copyRawError(): Promise<void> {
    if (!rawErrorDetail || typeof navigator?.clipboard?.writeText !== "function") {
      setCopyState("failed");
      return;
    }
    try {
      await navigator.clipboard.writeText(rawErrorDetail);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  }

  return (
    <article
      className={`tc-message tc-message--${item.kind}${isFailedUserMessage ? " tc-message--user-failed" : ""}${isPendingUserMessage ? " tc-message--user-pending" : ""}${isAbandonedUserMessage ? " tc-message--user-abandoned" : ""}`}
      data-delivery-state={item.deliveryState}
      data-kind={item.kind}
      data-message-id={item.id}
      data-message-kind={item.kind}
      data-testid="message-block"
      onClick={(event) => {
        if (!onEdit || window.getSelection()?.toString() || (event.target as Element).closest('button,a,[role="button"]')) return;
        onEdit(item);
      }}
      data-editable={!!onEdit}
      data-has-writes={!!hasFileWrites}
    >
      {onEdit ? (
        <button
          type="button"
          className={`tc-message__edit-trigger${hasFileWrites ? " tc-message__edit-trigger--revert" : ""}`}
          data-testid="edit-user-message"
          aria-label={t("message.edit")}
          title={t("message.edit")}
          onClick={() => onEdit(item)}
        >
          <span aria-hidden="true" className={`codicon ${hasFileWrites ? "codicon-discard" : "codicon-edit"}`} />
        </button>
      ) : null}
      {showHeader ? (
        <div className="tc-message__header">
          <strong>{t(`message.label.${item.kind}`)}</strong>
          <span>{item.label ?? t(`message.kind.${item.kind}`)}</span>
        </div>
      ) : null}
      <div className="message-text rendered-markdown" data-testid="message-text">
        {item.kind === "assistant" ? (
          <ChatMarkdown
            markdown={item.text}
            mediaRoots={mediaRoots}
            onOpenFile={onOpenFile ?? NOOP_OPEN_FILE}
            onOpenLink={onOpenLink}
            resolvePaths={resolvePaths}
            onZoomImage={onZoomImage}
          />
        ) : (
          segments.map((segment, index) =>
            segment.type === "text" ? (
              <span className="tc-message__text-segment" key={`${item.id}-text-${index}`}>
                {segment.text}
              </span>
            ) : segment.type === "instruction" ? (
              <InvocationChip key={`${item.id}-instruction-${index}`} instruction={segment} />
            ) : (
              <ReferenceChip
                key={`${item.id}-reference-${index}`}
                reference={segment}
                testId="history-reference-chip"
              />
            ),
          )
        )}
      </div>
      <AttachmentStrip
        attachments={historyAttachments}
        onOpen={(attachment) => {
          if (attachment.kind === "image") {
            onOpenImagePreview?.(attachment.id);
            return;
          }
          if (attachment.path) {
            onOpenFile?.(attachment.path);
          }
        }}
        readonly
      />
      {canToggleRawError || recoveryAction ? (
        <>
          {canToggleRawError ? (
            <div className="tc-message__detail-actions" data-testid="error-detail-actions">
              <button
                aria-expanded={detailsExpanded}
                className="tc-message__detail-button"
                data-testid="toggle-error-detail"
                onClick={() => setDetailsExpanded((value) => !value)}
                type="button"
              >
                <span
                  aria-hidden="true"
                  className={`codicon ${detailsExpanded ? "codicon-chevron-down" : "codicon-chevron-right"}`}
                />
                <span>{t(detailsExpanded ? "message.hideOriginal" : "message.showOriginal")}</span>
              </button>
              <button
                className="tc-message__detail-button"
                data-testid="copy-error-detail"
                onClick={() => {
                  void copyRawError();
                }}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-copy" />
                <span>
                  {t(copyState === "copied" ? "common.copied" : copyState === "failed" ? "common.copyFailed" : "message.copyOriginal")}
                </span>
              </button>
            </div>
          ) : null}
          {recoveryAction ? (
            <div className="tc-message__recovery-actions" data-testid="error-recovery-actions">
              <button
                className="tc-button tc-button--primary tc-message__recovery-button"
                data-testid="recover-error-turn"
                disabled={recoveryDisabled}
                onClick={() => onRecover?.(item.id, recoveryAction)}
                type="button"
              >
                <span
                  aria-hidden="true"
                  className={`codicon ${recoveryAction === "retry" ? "codicon-refresh" : "codicon-debug-continue"}`}
                />
                <span>{t(recoveryAction === "retry" ? "common.retry" : "common.resume")}</span>
              </button>
            </div>
          ) : null}
        </>
      ) : null}
      {item.kind === "error" && item.recoveryError ? (
        <div className="tc-message__status" data-testid="error-recovery-rejection">
          <span>{item.recoveryError}</span>
        </div>
      ) : null}
      {detailsExpanded && rawErrorDetail ? (
        <pre className="tc-message__detail" data-testid="error-detail-text">
          {rawErrorDetail}
        </pre>
      ) : null}
      {isPendingUserMessage ? (
        <div className="tc-message__status" data-testid="user-message-status">
          <span>{t("message.sending")}</span>
        </div>
      ) : null}
      {isAbandonedUserMessage ? (
        <div className="tc-message__status" data-testid="abandoned-user-message-status">
          <span>{t("message.abandoned")}</span>
        </div>
      ) : null}
      {isFailedUserMessage ? (
        <div className="tc-message__status" data-testid="user-message-status">
          <span title={item.deliveryErrorDetail ?? undefined}>
            {item.deliveryError ?? t("message.sendFailed")}
          </span>
          {showRetry ? (
            <button
              className="tc-message__retry"
              data-testid="retry-user-message"
              onClick={() => onRetry?.(item.id)}
              type="button"
            >
              {t("common.retry")}
            </button>
          ) : null}
        </div>
      ) : null}
    </article>
  );
}

function areMessageBubblePropsEqual(prev: MessageBubbleProps, next: MessageBubbleProps): boolean {
  return (
    prev.mediaRoots === next.mediaRoots &&
    prev.onEdit === next.onEdit &&
    prev.hasFileWrites === next.hasFileWrites &&
    prev.item === next.item &&
    prev.onOpenFile === next.onOpenFile &&
    prev.onOpenImagePreview === next.onOpenImagePreview &&
    prev.onRecover === next.onRecover &&
    prev.onRetry === next.onRetry &&
    prev.recoveryDisabled === next.recoveryDisabled &&
    prev.onZoomImage === next.onZoomImage
  );
}

export const MessageBubble = memo(MessageBubbleComponent, areMessageBubblePropsEqual);
MessageBubble.displayName = "MessageBubble";
