import { ConfirmationDialog } from "./ConfirmationDialog";
import type { PreviewRewindResponse } from "../../../src/serveClient/wire";

export const REVERT_REASONS = {
  no_baselines: "这条消息没有可用的文件备份，只能保留文件重发。",
  expired: "这条消息的文件备份已过期，只能保留文件重发。",
  git_head_moved: "这条消息之后有过 Git 提交或切换过分支，不能恢复文件。",
};

export function EditConfirmDialog({ preview, busy, onCancel, onChoose }: {
  preview: PreviewRewindResponse;
  busy: boolean;
  onCancel(): void;
  onChoose(files: "keep" | "revert"): void;
}) {
  const reason = preview.revertReason ? REVERT_REASONS[preview.revertReason] : undefined;
  return (
    <ConfirmationDialog
      testId="edit-confirm"
      title="重新发送这条消息？"
      body={`重发将替换本条消息及后续对话。${busy ? "当前任务会先停止。" : ""}${preview.revertAvailable ? `Revert Files：把 AI 在本条及之后用写文件工具改过的 ${preview.revertPaths.length} 个文件恢复原样（它新建的会删除，也会覆盖这些文件后续的修改）；终端改动不会单独撤销。` : reason ?? "没有可用的文件备份。"}`}
      actions={[
        { id: "keep", label: "Keep Files", shortcut: "⇧↵", tone: "secondary" },
        {
          id: "revert",
          label: "Revert Files",
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
