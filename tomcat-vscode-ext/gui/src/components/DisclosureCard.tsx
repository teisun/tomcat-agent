import { useEffect, useState, type ReactNode } from "react";

export type DisclosureStatusVariant = "error" | "neutral" | "running" | "success" | "warning";

export function DisclosureCard({
  bodyTestId,
  children,
  defaultExpanded = false,
  header,
  leadingIcon,
  preview,
  resetKey,
  statusVariant,
  toggleTestId,
}: {
  bodyTestId?: string;
  children?: ReactNode;
  defaultExpanded?: boolean;
  header: ReactNode;
  leadingIcon?: ReactNode;
  preview?: ReactNode;
  resetKey?: string;
  statusVariant: DisclosureStatusVariant;
  toggleTestId?: string;
}) {
  const canToggle = Boolean(preview ?? children);
  const [expanded, setExpanded] = useState(defaultExpanded);
  const [userInteracted, setUserInteracted] = useState(false);

  useEffect(() => {
    setExpanded(defaultExpanded);
    setUserInteracted(false);
  }, [defaultExpanded, resetKey]);

  useEffect(() => {
    if (!userInteracted) {
      setExpanded(defaultExpanded);
    }
  }, [defaultExpanded, userInteracted]);

  return (
    <section
      className={`tc-disclosure-card tc-disclosure-card--${statusVariant}`}
      data-testid="disclosure-card"
    >
      <span aria-hidden="true" className="tc-disclosure-card__status" />
      <div className="tc-disclosure-card__header">
        {leadingIcon ? (
          <span
            aria-hidden="true"
            className="tc-disclosure-card__leading"
            data-testid="disclosure-card-leading-icon"
          >
            {leadingIcon}
          </span>
        ) : null}
        <div className="tc-disclosure-card__header-content">{header}</div>
        {canToggle ? (
          <button
            aria-expanded={expanded}
            aria-label={expanded ? "Collapse tool result" : "Expand tool result"}
            className="tc-disclosure-card__toggle"
            data-testid={toggleTestId}
            onClick={() => {
              setUserInteracted(true);
              setExpanded((value) => !value);
            }}
            type="button"
          >
            <span className="tc-disclosure-card__caret">{expanded ? "▾" : "▸"}</span>
          </button>
        ) : null}
      </div>
      {expanded ? (
        children ? (
          <div className="tc-disclosure-card__body" data-testid={bodyTestId}>
            {children}
          </div>
        ) : null
      ) : preview ? (
        <div className="tc-disclosure-card__preview" data-testid="disclosure-card-preview">
          {preview}
        </div>
      ) : null}
    </section>
  );
}
