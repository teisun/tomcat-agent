import type { SessionFile } from "../serveClient/wire";

export type SessionFileView = SessionFile & { displayPath?: string };
export interface SessionFilesView {
  sourceTurnId: string | null;
  files: SessionFileView[];
  error?: string;
}
export type SessionFileIntent =
  | { messageId: string; type: "refreshSessionFiles"; data: { sessionId: string } }
  | { messageId: string; type: "openSessionFileDiff"; data: { sessionId: string; sourceTurnId: string; path: string } }
  | { messageId: string; type: "keepSessionFiles"; data: { sessionId: string; sourceTurnId: string; requestId: string } }
  | { messageId: string; type: "restoreSessionFiles"; data: { sessionId: string; sourceTurnId: string; paths: string[]; requestId: string } };
export interface SessionFilesResult {
  type: "restoreSessionFilesResult" | "keepSessionFilesResult";
  sessionId: string;
  sourceTurnId: string;
  requestId: string;
  success: boolean;
  error?: string;
}
