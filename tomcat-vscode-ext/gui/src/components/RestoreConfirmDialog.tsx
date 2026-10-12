import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, type Locale, type Translator } from "../../../src/shared/i18n";

import { ConfirmationDialog } from "./ConfirmationDialog";

function basename(filePath: string): string {
  const segments = filePath.split(/[\\/]/);
  return segments[segments.length - 1] || filePath;
}

function describeChangedFiles(changedFiles: string[], t: Translator, locale: Locale): string {
  if (changedFiles.length === 1) return t("checkpoint.file", { name: basename(changedFiles[0]) });
  if (changedFiles.length > 1) return t(pluralKey(locale, "checkpoint.files.other", changedFiles.length), { count: changedFiles.length });
  return t("checkpoint.files.unknown");
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
  const t = useT();
  const locale = useLocale();
  const body = t("checkpoint.confirm", { files: describeChangedFiles(changedFiles, t, locale) });

  return (
    <ConfirmationDialog
      actions={[
        { id: "dont-revert", label: t("checkpoint.keep"), shortcut: "⇧↵", tone: "secondary" },
        { id: "revert", label: t("checkpoint.revert"), shortcut: "↵", tone: "primary" },
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
      title={t("checkpoint.restore")}
    />
  );
}
