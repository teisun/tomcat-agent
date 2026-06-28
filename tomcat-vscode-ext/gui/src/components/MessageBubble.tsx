import type { WebviewMessageBlock } from "../types";

const MESSAGE_LABELS: Record<WebviewMessageBlock["kind"], string> = {
  assistant: "Tomcat",
  error: "Error",
  notice: "Notice",
  user: "You",
  warn: "Warn",
};

export function MessageBubble({ item }: { item: WebviewMessageBlock }) {
  const showHeader = item.kind !== "user" && item.kind !== "assistant";

  return (
    <article
      className={`tc-message tc-message--${item.kind}`}
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
        {item.text.split("\n\n").map((paragraph, index) => (
          <p key={`${item.id}-${index}`}>{paragraph}</p>
        ))}
      </div>
    </article>
  );
}
