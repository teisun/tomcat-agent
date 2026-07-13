import { useEffect, useMemo, useRef } from "react";

function basename(filePath: string): string {
  const segments = filePath.split(/[\\/]/);
  return segments[segments.length - 1] || filePath;
}

function describeChangedFiles(changedFiles: string[]): string {
  if (changedFiles.length === 1) {
    return `1 changed file (${basename(changedFiles[0])})`;
  }
  if (changedFiles.length > 1) {
    return `${changedFiles.length} changed files`;
  }
  return "your changed files";
}

export function RestoreConfirmDialog({
  changedFiles,
  onCancel,
  onDontRevert,
  onRevert,
}: {
  changedFiles: string[];
  onCancel(): void;
  onDontRevert(): void;
  onRevert(): void;
}) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const revertButtonRef = useRef<HTMLButtonElement | null>(null);
  const body = useMemo(() => {
    const changedFilesText = describeChangedFiles(changedFiles);
    return `Revert rolls back ${changedFilesText} to this point and clears every message after it. Don't revert keeps your current files and only clears those messages.`;
  }, [changedFiles]);

  useEffect(() => {
    revertButtonRef.current?.focus();
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        event.stopPropagation();
        if (event.shiftKey) {
          onDontRevert();
          return;
        }
        onRevert();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])");
      if (!focusable?.length) {
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (event.shiftKey && active === first) {
        event.preventDefault();
        last.focus();
        return;
      }
      if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
    };
  }, [onCancel, onDontRevert, onRevert]);

  return (
    <div
      className="tc-confirm-dialog__overlay"
      data-testid="cp-confirm-overlay"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onCancel();
        }
      }}
    >
      <div
        aria-labelledby="cp-confirm-title"
        aria-modal="true"
        className="tc-confirm-dialog"
        data-testid="cp-confirm-dialog"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
      >
        <div className="tc-confirm-dialog__header">
          <h3 className="tc-confirm-dialog__title" id="cp-confirm-title">
            Restore to this checkpoint?
          </h3>
        </div>
        <p className="tc-confirm-dialog__body" data-testid="cp-confirm-body">
          {body}
        </p>
        <div className="tc-confirm-dialog__actions">
          <button
            className="tc-confirm-dialog__button tc-confirm-dialog__button--ghost"
            data-testid="cp-confirm-cancel"
            onClick={onCancel}
            type="button"
          >
            <span>Cancel</span>
            <span className="tc-confirm-dialog__shortcut">Esc</span>
          </button>
          <button
            className="tc-confirm-dialog__button tc-confirm-dialog__button--secondary"
            data-testid="cp-confirm-dont-revert"
            onClick={onDontRevert}
            type="button"
          >
            <span>Don't revert</span>
            <span className="tc-confirm-dialog__shortcut">⇧↵</span>
          </button>
          <button
            className="tc-confirm-dialog__button tc-confirm-dialog__button--primary"
            data-testid="cp-confirm-revert"
            onClick={onRevert}
            ref={revertButtonRef}
            type="button"
          >
            <span>Revert</span>
            <span className="tc-confirm-dialog__shortcut">↵</span>
          </button>
        </div>
      </div>
    </div>
  );
}
