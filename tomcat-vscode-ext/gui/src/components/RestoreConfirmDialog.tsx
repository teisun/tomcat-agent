import { useMemo } from "react";

import { ConfirmationDialog } from "./ConfirmationDialog";

function basename(filePath: string): string {
  const segments = filePath.split(/[\\/]/);
  return segments[segments.length - 1] || filePath;
}

function describeChangedFiles(changedFiles: string[]): string {
  if (changedFiles.length === 1) return `1 changed file (${basename(changedFiles[0])})`;
  if (changedFiles.length > 1) return `${changedFiles.length} changed files`;
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
  const body = useMemo(() => {
    const changedFilesText = describeChangedFiles(changedFiles);
    return `Revert rolls back ${changedFilesText} to this point and clears every message after it. Don't revert keeps your current files and only clears those messages.`;
  }, [changedFiles]);

  return (
    <ConfirmationDialog
      actions={[
        { id: "dont-revert", label: "Don't revert", shortcut: "⇧↵", tone: "secondary" },
        { id: "revert", label: "Revert", shortcut: "↵", tone: "primary" },
      ]}
      body={body}
      onAction={(actionId) => {
        if (actionId === "revert") onRevert();
        else onDontRevert();
      }}
      onCancel={onCancel}
      onKeyDown={(event) => {
        if (event.key !== "Enter") return;
        event.preventDefault();
        event.stopPropagation();
        if (event.shiftKey) onDontRevert();
        else onRevert();
      }}
      primaryActionId="revert"
      testId="cp-confirm"
      title="Restore to this checkpoint?"
    />
  );
}
