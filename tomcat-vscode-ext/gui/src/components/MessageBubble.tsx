import type { WebviewMessageBlock } from "../types";

const MESSAGE_LABELS: Record<WebviewMessageBlock["kind"], string> = {
  assistant: "Tomcat",
  error: "Error",
  notice: "Notice",
  user: "You",
};

export function MessageBubble({ item }: { item: WebviewMessageBlock }) {
  return (
    <article
      className={`tc-message tc-message--${item.kind}`}
      data-kind={item.kind}
      data-testid="message-block"
    >
      <div className="tc-message__header">
        <strong>{MESSAGE_LABELS[item.kind]}</strong>
        <span>{item.kind}</span>
      </div>
      <p data-testid="message-text">{item.text}</p>
    </article>
  );
}
