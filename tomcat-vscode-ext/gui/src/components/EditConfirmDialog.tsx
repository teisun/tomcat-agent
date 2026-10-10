import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, type MessageKey } from "../../../src/shared/i18n";
import { ConfirmationDialog } from "./ConfirmationDialog";
import type { PreviewRewindResponse } from "../../../src/serveClient/wire";

export const REVERT_REASONS = {
  no_baselines: "edit.noBaselines",
  expired: "edit.expired",
  git_head_moved: "edit.gitMoved",
} satisfies Record<NonNullable<PreviewRewindResponse["revertReason"]>, MessageKey>;

export function EditConfirmDialog({ preview, busy, onCancel, onChoose }: {
  preview: PreviewRewindResponse;
  busy: boolean;
  onCancel(): void;
  onChoose(files: "keep" | "revert"): void;
}) {
  const t = useT();
  const locale = useLocale();
  const reason = preview.revertReason ? t(REVERT_REASONS[preview.revertReason]) : undefined;
  return (
    <ConfirmationDialog
      testId="edit-confirm"
      title={t("edit.title")}
      body={t("edit.body", { stopNotice: busy ? t("edit.stopNotice") : "", fileNotice: preview.revertAvailable
        ? t(pluralKey(locale, "edit.fileNotice.other", preview.revertPaths.length), { count: preview.revertPaths.length }) : reason ?? t("edit.noBackups") })}
      actions={[
        { id: "keep", label: t("edit.keepFiles"), shortcut: "⇧↵", tone: "secondary" },
        {
          id: "revert",
          label: t("edit.revertFiles"),
          shortcut: "↵",
          tone: "primary",
          disabled: !preview.revertAvailable,
          reason,
        },
      ]}
      primaryActionId="revert"
      onCancel={onCancel}
      onAction={(id) => {
        if (id === "keep" || preview.revertAvailable) onChoose(id === "keep" ? "keep" : "revert");
      }}
      onKeyDown={(event) => {
        if (event.key !== "Enter" || event.isComposing) return;
        event.preventDefault();
        event.stopPropagation();
        const focusedAction = event.target instanceof HTMLButtonElement
          ? event.target.dataset.testid
          : undefined;
        if (event.shiftKey || focusedAction === "edit-confirm-keep") onChoose("keep");
        else if (focusedAction === "edit-confirm-cancel") onCancel();
        else if (preview.revertAvailable) onChoose("revert");
      }}
    />
  );
}
