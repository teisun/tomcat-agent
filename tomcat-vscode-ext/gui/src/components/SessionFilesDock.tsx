import { useEffect, useRef, useState } from "react";
import type { SessionFileIntent, SessionFilesView, SessionFileView } from "../../../src/shared/sessionFiles";
import { DockSection } from "./DockSection";
import { SessionFilesList } from "./SessionFilesList";
import { UndoFilesConfirmDialog } from "./UndoFilesConfirmDialog";

export function SessionFilesDock({ sessionId, files, busy, onIntent }: {
  sessionId: string; files?: SessionFilesView; busy: boolean; onIntent(intent: SessionFileIntent): void;
}) {
  const [confirmation, setConfirmation] = useState<{ files: SessionFileView[]; skipped: number } | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const sourceTurnId = files?.sourceTurnId;
  useEffect(() => { setConfirmation(null); setPending(null); setError(null); if (listRef.current) listRef.current.scrollTop = 0; }, [sourceTurnId, sessionId]);
  useEffect(() => {
    function receive(event: MessageEvent) {
      const result = event.data?.channel === "event" ? event.data.content : null;
      if (!result || !["restoreSessionFilesResult", "keepSessionFilesResult"].includes(result.type) || result.sessionId !== sessionId || result.sourceTurnId !== sourceTurnId || result.requestId !== pending) return;
      setPending(null); setError(result.success ? null : result.error ?? "Couldn't update file changes.");
    }
    window.addEventListener("message", receive);
    return () => window.removeEventListener("message", receive);
  }, [pending, sessionId, sourceTurnId]);
  if (!files?.files.length && !files?.error && !pending && !error) return null;
  const rows = files?.files ?? [];
  const eligible = rows.filter(file => file.restorable);
  const disabled = busy || !!pending;
  function refresh() { onIntent({ messageId: crypto.randomUUID(), type: "refreshSessionFiles", data: { sessionId } }); }
  function keep() {
    if (disabled || !rows.length || !sourceTurnId) return;
    const requestId = crypto.randomUUID(); setPending(requestId); setError(null);
    onIntent({ messageId: requestId, type: "keepSessionFiles", data: { sessionId, sourceTurnId, requestId } });
  }
  function confirm() {
    if (!confirmation || disabled || !sourceTurnId) return;
    const requestId = crypto.randomUUID(); setPending(requestId); setError(null);
    onIntent({ messageId: requestId, type: "restoreSessionFiles", data: { sessionId, sourceTurnId, paths: confirmation.files.map(file => file.path), requestId } });
    setConfirmation(null);
  }
  return <>
    <DockSection title={`${rows.length} Files`} label="files" testId="files-dock" toggleTestId="files-toggle" titleTestId="files-title"
      tooltip={`Changes since the last Keep All (or session start). Committed changes count as kept.${files?.error ? ` · Refresh failed: ${files.error}` : ""}`} onExpand={refresh}
      actions={<><button type="button" data-testid="keep-all-files" disabled={disabled || !rows.length || !sourceTurnId} title="Accept all current changes and start a new list. Files on disk are not changed." onClick={keep}>Keep All</button>
        <button type="button" data-testid="undo-all-files" disabled={disabled || !eligible.length} title={disabled ? "Stop Tomcat or wait for it to finish to undo." : !eligible.length ? "No files can be undone." : "Undo restorable files changed since the last Keep All"}
        onClick={() => setConfirmation({ files: eligible, skipped: rows.length - eligible.length })}>Undo All</button></>}>
      {sourceTurnId ? <SessionFilesList sessionId={sessionId} sourceTurnId={sourceTurnId} files={rows} busy={disabled} onIntent={onIntent} listRef={listRef}
        onUndo={file => setConfirmation({ files: [file], skipped: 0 })} /> : null}
    </DockSection>
    {files?.error && !rows.length ? <div className="tc-session-dock__error" role="status">Couldn't load file changes. <button type="button" onClick={refresh}>Retry</button></div> : null}
    {error ? <div className="tc-session-files__error" role="alert">{error}</div> : null}
    {confirmation ? <UndoFilesConfirmDialog files={confirmation.files} skipped={confirmation.skipped} busy={disabled} onCancel={() => setConfirmation(null)} onConfirm={confirm} /> : null}
  </>;
}
