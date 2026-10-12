import { useEffect, type RefObject } from "react";
import { useT } from "../i18n/LocaleProvider";

export interface PlanFindWidgetProps {
  activeIndex: number;
  inputRef?: RefObject<HTMLInputElement | null>;
  onClose(): void;
  onNext(): void;
  onPrevious(): void;
  onQueryChange(query: string): void;
  query: string;
  total: number;
}

/**
 * In-webview Find control for Plan Preview. VS Code's webview Find widget does
 * not expose its current/total result count to extensions, so this component
 * owns the visible Cursor-style "N of M" label.
 */
export function PlanFindWidget({
  activeIndex,
  inputRef,
  onClose,
  onNext,
  onPrevious,
  onQueryChange,
  query,
  total,
}: PlanFindWidgetProps) {
  const t = useT();
  useEffect(() => {
    inputRef?.current?.focus();
    inputRef?.current?.select();
  }, [inputRef]);

  const hasQuery = query.length > 0;
  const hasMatches = total > 0;
  const inputIsInvalid = hasQuery && !hasMatches;
  const countLabel = hasQuery
    ? hasMatches
      ? t("find.count", { current: activeIndex + 1, total })
      : t("find.none")
    : null;

  return (
    <section
      aria-label={t("find.plan")}
      className="tc-plan-find"
      data-testid="plan-find"
    >
      <input
        aria-label={t("find.plan")}
        aria-invalid={inputIsInvalid}
        className={`tc-plan-find__input${inputIsInvalid ? " tc-plan-find__input--empty" : ""}`}
        data-testid="plan-find-input"
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            if (event.shiftKey) {
              onPrevious();
            } else {
              onNext();
            }
          }
        }}
        ref={inputRef}
        spellCheck={false}
        type="text"
        value={query}
      />
      {countLabel ? (
        <span
          aria-live="polite"
          className={`tc-plan-find__count${hasMatches ? "" : " tc-plan-find__count--empty"}`}
          data-testid="plan-find-count"
        >
          {countLabel}
        </span>
      ) : null}
      <button
        aria-label={t("find.previous")}
        className="tc-plan-find__button"
        data-testid="plan-find-prev"
        disabled={!hasMatches}
        onClick={onPrevious}
        type="button"
      >
        <span aria-hidden="true" className="codicon codicon-arrow-up" />
      </button>
      <button
        aria-label={t("find.next")}
        className="tc-plan-find__button"
        data-testid="plan-find-next"
        disabled={!hasMatches}
        onClick={onNext}
        type="button"
      >
        <span aria-hidden="true" className="codicon codicon-arrow-down" />
      </button>
      <button
        aria-label={t("find.close")}
        className="tc-plan-find__button"
        data-testid="plan-find-close"
        onClick={onClose}
        type="button"
      >
        <span aria-hidden="true" className="codicon codicon-close" />
      </button>
    </section>
  );
}
