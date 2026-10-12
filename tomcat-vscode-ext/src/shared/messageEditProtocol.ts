import { t } from "./i18n";
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
  if (error === "stop_timeout") return t("rewind.stopTimeout");
  if (error === "busy") return t("rewind.busy");
  if (error === "rewind_target_ineligible") return t("rewind.ineligible");
  if (error === "revert_failed" && payload && typeof payload === "object") {
    const details = payload as { path?: unknown; reason?: unknown };
    if (typeof details.path === "string" && typeof details.reason === "string") {
      return t("rewind.restoreFailed", { path: details.path, reason: details.reason });
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
