import type { SessionFileView } from "../../../src/shared/sessionFiles";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey } from "../../../src/shared/i18n";
import { ConfirmationDialog } from "./ConfirmationDialog";
const basename = (path: string) => path.split(/[\\/]/).at(-1) || path;

export function UndoFilesConfirmDialog({ files, skipped, onCancel, onConfirm, busy }: {
  files: SessionFileView[]; skipped: number; onCancel(): void; onConfirm(): void; busy: boolean;
}) {
  const t = useT();
  const locale = useLocale();
  const shortNames = files.slice(0, 5).map(file => basename(file.path)).join(", ");
  const names = files.length > 5 ? t(pluralKey(locale, "undo.more.other", files.length - 5), { names: shortNames, count: files.length - 5 }) : shortNames;
  const single = files.length === 1;
  const text = t(single && files[0].status === "added" ? "undo.added" : single ? "undo.single" : "undo.multiple", { names });
  const skippedText = skipped ? t(pluralKey(locale, "undo.skipped.other", skipped), { count: skipped }) : "";
  return <ConfirmationDialog testId="undo-files"
    title={single ? t("undo.title", { name: names }) : t(pluralKey(locale, "undo.titleCount.other", files.length), { count: files.length })}
    body={skippedText ? `${text} ${skippedText}` : text}
    actions={[{ id: "undo", label: single ? t("undo.file") : t(pluralKey(locale, "undo.count.other", files.length), { count: files.length }),
      disabled: busy, reason: busy ? t("files.stopBeforeUndo") : undefined, shortcut: "↵", tone: "primary" }]}
    primaryActionId="undo" onCancel={onCancel} onAction={onConfirm}
    onKeyDown={event => { if (event.key === "Enter" && !busy) { event.preventDefault(); event.stopPropagation(); onConfirm(); } }} />;
}
