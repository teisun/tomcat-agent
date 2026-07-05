import { ReferenceChip } from "./ReferenceChip";
import type { WebviewMessageBlock, WebviewMessageSegment } from "../types";

const MESSAGE_LABELS: Record<WebviewMessageBlock["kind"], string> = {
  assistant: "Tomcat",
  error: "Error",
  notice: "Notice",
  user: "You",
  warn: "Warn",
};

export function MessageBubble({
  item,
  onRetry,
}: {
  item: WebviewMessageBlock;
  onRetry?: (messageId: string) => void;
}) {
  const showHeader = item.kind !== "user" && item.kind !== "assistant";
  const isFailedUserMessage = item.kind === "user" && item.deliveryState === "failed";
  const isPendingUserMessage = item.kind === "user" && item.deliveryState === "pending";
  const showRetry = isFailedUserMessage && item.retryable === true && typeof onRetry === "function";
  const segments: WebviewMessageSegment[] =
    item.segments?.length ? item.segments : [{ text: item.text, type: "text" }];

  return (
    <article
      className={`tc-message tc-message--${item.kind}${isFailedUserMessage ? " tc-message--user-failed" : ""}${isPendingUserMessage ? " tc-message--user-pending" : ""}`}
      data-delivery-state={item.deliveryState}
      data-kind={item.kind}
      data-message-id={item.id}
      data-message-kind={item.kind}
      data-testid="message-block"
    >
      {showHeader ? (
        <div className="tc-message__header">
          <strong>{MESSAGE_LABELS[item.kind]}</strong>
          <span>{item.kind}</span>
        </div>
      ) : null}
      <div className="message-text rendered-markdown" data-testid="message-text">
        {segments.map((segment, index) =>
          segment.type === "text" ? (
            <span className="tc-message__text-segment" key={`${item.id}-text-${index}`}>
              {segment.text}
            </span>
          ) : (
            <ReferenceChip
              key={`${item.id}-reference-${index}`}
              reference={segment}
              testId="history-reference-chip"
            />
          ),
        )}
      </div>
      {isPendingUserMessage ? (
        <div className="tc-message__status" data-testid="user-message-status">
          <span>Sending...</span>
        </div>
      ) : null}
      {isFailedUserMessage ? (
        <div className="tc-message__status" data-testid="user-message-status">
          <span>{item.deliveryError ?? "Send failed."}</span>
          {showRetry ? (
            <button
              className="tc-message__retry"
              data-testid="retry-user-message"
              onClick={() => onRetry?.(item.id)}
              type="button"
            >
              Retry
            </button>
          ) : null}
        </div>
      ) : null}
    </article>
  );
}
