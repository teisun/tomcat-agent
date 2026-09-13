import { useEffect, useRef } from "react";

export type ConfirmationAction = {
  id: string;
  label: string;
  shortcut?: string;
  tone?: "ghost" | "secondary" | "primary";
};

export function ConfirmationDialog({
  actions,
  ariaLabelledBy,
  body,
  cancelLabel = "Cancel",
  onAction,
  onCancel,
  onKeyDown,
  testId = "confirmation",
  title,
  primaryActionId,
}: {
  actions: ConfirmationAction[];
  ariaLabelledBy?: string;
  body: string;
  cancelLabel?: string;
  onAction(actionId: string): void;
  onCancel(): void;
  onKeyDown?(event: KeyboardEvent): void;
  primaryActionId?: string;
  testId?: string;
  title: string;
}) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const primaryButtonRef = useRef<HTMLButtonElement | null>(null);
  const titleId = ariaLabelledBy ?? `${testId}-title`;

  useEffect(() => {
    primaryButtonRef.current?.focus();
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
        return;
      }
      if (event.key === "Tab") {
        const focusable = dialogRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])");
        if (!focusable?.length) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        const active = document.activeElement as HTMLElement | null;
        if (event.shiftKey && active === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && active === last) {
          event.preventDefault();
          first.focus();
        }
        return;
      }
      onKeyDown?.(event);
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [onCancel, onKeyDown]);

  return (
    <div
      className="tc-confirm-dialog__overlay"
      data-testid={`${testId}-overlay`}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <div
        aria-labelledby={titleId}
        aria-modal="true"
        className="tc-confirm-dialog"
        data-testid={`${testId}-dialog`}
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
      >
        <div className="tc-confirm-dialog__header">
          <h3 className="tc-confirm-dialog__title" id={titleId}>
            {title}
          </h3>
        </div>
        <p className="tc-confirm-dialog__body" data-testid={`${testId}-body`}>
          {body}
        </p>
        <div className="tc-confirm-dialog__actions">
          <button
            className="tc-confirm-dialog__button tc-confirm-dialog__button--ghost"
            data-testid={`${testId}-cancel`}
            onClick={onCancel}
            type="button"
          >
            <span>{cancelLabel}</span>
            <span className="tc-confirm-dialog__shortcut">Esc</span>
          </button>
          {actions.map((action) => (
            <button
              className={`tc-confirm-dialog__button tc-confirm-dialog__button--${action.tone ?? "secondary"}`}
              data-testid={`${testId}-${action.id}`}
              key={action.id}
              onClick={() => onAction(action.id)}
              ref={action.id === primaryActionId ? primaryButtonRef : undefined}
              type="button"
            >
              <span>{action.label}</span>
              {action.shortcut ? <span className="tc-confirm-dialog__shortcut">{action.shortcut}</span> : null}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
