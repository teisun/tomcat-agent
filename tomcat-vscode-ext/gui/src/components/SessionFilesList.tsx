import { useEffect, useState, type RefObject } from "react";
import type { SessionFileIntent, SessionFileView } from "../../../src/shared/sessionFiles";
import { UndoFilesConfirmDialog } from "./UndoFilesConfirmDialog";

function split(path: string) { return path.split(/[\\/]/).filter(Boolean); }
function basename(path: string) { return split(path).at(-1) || path; }
function directoryLabel(file: SessionFileView, files: SessionFileView[]): string {
  const peers = files.filter((other) => other.path !== file.path && basename(other.path) === basename(file.path));
  if (!peers.length) return "";
  const parts = split(file.displayPath ?? file.path).slice(0,-1);
  for (let length=1; length<=parts.length; length+=1) {
    const suffix=parts.slice(-length).join("/");
    if (peers.every((other) => split(other.displayPath ?? other.path).slice(0,-1).slice(-length).join("/") !== suffix)) return suffix;
  }
  return parts.join("/");
}
function blockedReason(file: SessionFileView, busy: boolean) {
  if (busy) return "Stop Tomcat or wait for it to finish to undo.";
  if (file.blockedReason === "head_moved") return "Git HEAD changed since this backup. View the diff only.";
  if (file.blockedReason === "backup_missing") return "The original backup is missing. This file cannot be undone.";
  if (file.blockedReason === "not_regular_file") return "The path is not a regular file and cannot be undone.";
  return file.restorable ? "Undo this file" : "This file cannot be undone.";
}
export function SessionFilesList({ sessionId, sourceTurnId, files, busy, onIntent, listRef }: {
  sessionId: string; sourceTurnId: string; files: SessionFileView[]; busy: boolean; onIntent(intent: SessionFileIntent): void; listRef: RefObject<HTMLDivElement | null>;
}) {
  const [confirmation, setConfirmation] = useState<{ files: SessionFileView[]; skipped: number } | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { setConfirmation(null); setPending(null);setError(null); }, [sourceTurnId, sessionId]);
  useEffect(() => {
    function receive(event: MessageEvent) {
      const frame = event.data;
      const result = frame?.channel === "event" ? frame.content : null;
      if (result?.type !== "restoreSessionFilesResult" || result.sessionId !== sessionId || result.requestId !== pending) return;
      setPending(null);setError(result.success ? null : result.error ?? "Couldn't undo file changes.");
    }
    window.addEventListener("message",receive);return () => window.removeEventListener("message",receive);
  }, [pending,sessionId]);
  const disabled = busy || !!pending;
  const eligible = files.filter((file) => file.restorable);
  function confirm() {
    if (!confirmation || disabled) return;
    const requestId=crypto.randomUUID();setPending(requestId);setError(null);
    onIntent({messageId:requestId,type:"restoreSessionFiles",data:{sessionId,sourceTurnId,paths:confirmation.files.map((file) => file.path),requestId}});
    setConfirmation(null);
  }
  return <>
    <div className="tc-session-files__toolbar"><button type="button" data-testid="undo-all-files" disabled={disabled || !eligible.length} title={disabled ? blockedReason(files[0],true) : !eligible.length ? "No files can be undone." : "Undo restorable files in this editing turn"} onClick={() => setConfirmation({files:eligible,skipped:files.length-eligible.length})}>Undo All</button></div>
    <div className="tc-session-files__list" ref={listRef} data-testid="session-files-list" role="list" aria-label="Changed files">
      {files.map((file) => <div className="tc-session-files__row" key={file.path} role="listitem" data-testid="session-file-row" data-file-path={file.path}>
        <button type="button" className="tc-session-files__file" data-testid="session-file-diff" title={file.displayPath ?? file.path} onClick={() => onIntent({messageId:crypto.randomUUID(),type:"openSessionFileDiff",data:{sessionId,sourceTurnId,path:file.path}})}>
          <span className={`tc-session-files__name${file.status === "deleted" ? " tc-session-files__name--deleted" : ""}`}>{basename(file.path)}</span>
          {directoryLabel(file,files) ? <span className="tc-session-files__directory">{directoryLabel(file,files)}</span> : null}
          <span className="tc-session-files__stats" title={file.added == null || file.removed == null ? "Line counts unavailable: binary, oversized, or missing original." : !file.added && !file.removed ? "Only line endings or file existence changed." : undefined}>
            {file.added == null || file.removed == null || !file.added && !file.removed ? "—" : <>{file.added ? <span className="tc-session-files__added">+{file.added}</span> : null}{file.removed ? <span className="tc-session-files__removed">-{file.removed}</span> : null}</>}
          </span>
        </button>
        <span className="tc-session-files__undo-slot" title={blockedReason(file,disabled)}><button className="tc-session-files__undo" type="button" data-testid="undo-file" aria-label={`Undo ${basename(file.path)}`} disabled={disabled || !file.restorable} onClick={() => setConfirmation({files:[file],skipped:0})}><span aria-hidden="true" className="codicon codicon-close" /></button></span>
      </div>)}
    </div>
    {error ? <div className="tc-session-files__error" role="alert">{error}</div> : null}
    {confirmation ? <UndoFilesConfirmDialog files={confirmation.files} skipped={confirmation.skipped} busy={disabled} onCancel={() => setConfirmation(null)} onConfirm={confirm} /> : null}
  </>;
}
