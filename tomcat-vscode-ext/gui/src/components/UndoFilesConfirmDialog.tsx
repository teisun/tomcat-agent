import type { SessionFileView } from "../../../src/shared/sessionFiles";
import { ConfirmationDialog } from "./ConfirmationDialog";
const basename = (path: string) => path.split(/[\\/]/).at(-1) || path;
export function UndoFilesConfirmDialog({ files, skipped, onCancel, onConfirm, busy }: { files: SessionFileView[]; skipped: number; onCancel(): void; onConfirm(): void; busy: boolean }) {
  const names = files.slice(0,5).map((file) => basename(file.path)).join(", ") + (files.length > 5 ? ` and ${files.length - 5} more` : "");
  const single = files.length === 1;
  const text = single && files[0].status === "added"
    ? `${names} was created in the editing turn shown and will be deleted. Later saved changes, including your own edits, will be lost. Your chat is kept.`
    : `${single ? "Restores " + names : "Restores these files (" + names + ")"} to before the editing turn shown in Files. Later saved changes, including your own edits, will be lost. Files created in this turn will be deleted. Your chat is kept.`;
  const reason = "Stop Tomcat or wait for it to finish to undo.";
  return <ConfirmationDialog testId="undo-files" title={single ? `Undo changes to ${names}?` : `Undo changes to ${files.length} files?`} body={text + (skipped ? ` ${skipped} file${skipped === 1 ? "" : "s"} cannot be undone and will be left as is.` : "")} actions={[{id:"undo",label:single ? "Undo File" : `Undo ${files.length} Files`,disabled:busy,reason:busy ? reason : undefined,shortcut:"↵",tone:"primary"}]} primaryActionId="undo" onCancel={onCancel} onAction={onConfirm} onKeyDown={(event) => { if (event.key === "Enter" && !busy) { event.preventDefault();event.stopPropagation();onConfirm(); } }} />;
}
