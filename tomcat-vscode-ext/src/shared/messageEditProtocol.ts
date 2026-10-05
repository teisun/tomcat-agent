import type { PreviewRewindResponse, RewindFiles } from "../serveClient/wire";
import type { WebviewMessageSegment, WebviewPendingAttachment, WebviewReference } from "../ui/webview/protocol";

export type MessageEditIntent =
  | { messageId: string; type: "previewRewind"; data: { sessionId: string; messageId: string } }
  | { messageId: string; type: "rewindAndResend"; data: {
      sessionId: string; messageId: string; files: RewindFiles;
      text: string; segments: WebviewMessageSegment[]; attachments: WebviewPendingAttachment[];
    } };

export type MessageEditEvent = {
  type: "previewRewindResult" | "rewindAndResendResult";
  requestId: string; sessionId: string; success: boolean; error?: string;
  errorDetail?: string;
  preview?: PreviewRewindResponse;
} | {
  type: "editContextResult"; operationId: string; sessionId: string;
  attachments: WebviewPendingAttachment[]; references: WebviewReference[]; error?: string;
};

export function rewindErrorDetail(error: string | null | undefined, payload: unknown): string | undefined {
  if (error === "stop_timeout") return "停止当前任务超时，尚未重发；请等任务结束后再试。";
  if (error === "busy") return "会话仍在处理中，修改稿已保留，请稍后再试。";
  if (error === "rewind_target_ineligible") return "这条消息不是可编辑的独立用户轮次。";
  if (error === "revert_failed" && payload && typeof payload === "object") {
    const details = payload as { path?: unknown; reason?: unknown };
    if (typeof details.path === "string" && typeof details.reason === "string") {
      return `恢复 ${details.path} 失败：${details.reason}。文件可能已部分恢复；修改稿已保留。`;
    }
  }
  return undefined;
}

export function isPreviewRewindResponse(value: unknown): value is PreviewRewindResponse {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  return typeof v.revertAvailable === "boolean" && Array.isArray(v.revertPaths) && v.revertPaths.every((p) => typeof p === "string")
    && (v.revertReason === undefined || ["no_baselines", "expired", "git_head_moved"].includes(String(v.revertReason)));
}
