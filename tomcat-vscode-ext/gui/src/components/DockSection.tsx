import { useId, useState, type ReactNode } from "react";
import { useT } from "../i18n/LocaleProvider";

/** Layout only: business state and actions remain in the owning dock. */
export function DockSection({ title, label, children, actions, defaultExpanded = false, onExpand, testId, toggleTestId, titleTestId, titleClassName, tooltip }: {
  title: string | ((expanded: boolean) => string); label: "files" | "todos" | "messages"; children: ReactNode; actions?: ReactNode; defaultExpanded?: boolean;
  onExpand?(): void; testId?: string; toggleTestId?: string; titleTestId?: string; titleClassName?: string; tooltip?: string;
}) {
  const t = useT();
  const [expanded, setExpanded] = useState(defaultExpanded);
  const contentId = useId();
  return <section className="tc-session-dock tc-dock-section" data-testid={testId}>
    <div className="tc-session-dock__header">
      <button type="button" className="tc-session-dock__toggle" data-testid={toggleTestId}
        aria-label={t(expanded ? "dock.collapse" : "dock.expand", { label: t(`dock.label.${label}`) })} aria-expanded={expanded} aria-controls={contentId} title={tooltip}
        onClick={() => { if (!expanded) onExpand?.(); setExpanded(!expanded); }}>
        <span aria-hidden="true" className={`codicon codicon-chevron-${expanded ? "down" : "right"}`} />
        <span data-testid={titleTestId} className={`tc-session-dock__title ${titleClassName ?? ""}`}>{typeof title === "function" ? title(expanded) : title}</span>
      </button>
      {actions ? <div className="tc-dock-section__actions">{actions}</div> : null}
    </div>
    <div className="tc-dock-section__content" id={contentId} hidden={!expanded}>{children}</div>
  </section>;
}
