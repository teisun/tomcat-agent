import { useEffect, useRef, useState } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey } from "../../../src/shared/i18n";
import type { SessionFileIntent, SessionFilesView, SessionFileView } from "../../../src/shared/sessionFiles";
import { DockSection } from "./DockSection";
import { SessionFilesList } from "./SessionFilesList";
import { UndoFilesConfirmDialog } from "./UndoFilesConfirmDialog";

export function SessionFilesDock({ sessionId, files, busy, onIntent }: {
  sessionId: string; files?: SessionFilesView; busy: boolean; onIntent(intent: SessionFileIntent): void;
}) {
  const t = useT();
  const locale = useLocale();
  const [confirmation, setConfirmation] = useState<{ files: SessionFileView[]; skipped: number } | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | true | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const sourceTurnId = files?.sourceTurnId;
  useEffect(() => { setConfirmation(null); setPending(null); setError(null); if (listRef.current) listRef.current.scrollTop = 0; }, [sourceTurnId, sessionId]);
  useEffect(() => {
    function receive(event: MessageEvent) {
      const result = event.data?.channel === "event" ? event.data.content : null;
      if (!result || !["restoreSessionFilesResult", "keepSessionFilesResult"].includes(result.type) || result.sessionId !== sessionId || result.sourceTurnId !== sourceTurnId || result.requestId !== pending) return;
      setPending(null); setError(result.success ? null : result.error ?? true);
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
    <DockSection title={t(pluralKey(locale, "files.count.other", rows.length), { count: rows.length })} label="files" testId="files-dock" toggleTestId="files-toggle" titleTestId="files-title"
      tooltip={files?.error ? t("files.refreshFailed", { scope: t("files.scope"), detail: files.error }) : t("files.scope")} onExpand={refresh}
      actions={<><button type="button" data-testid="keep-all-files" disabled={disabled || !rows.length || !sourceTurnId} title={t("files.keepHint")} onClick={keep}>{t("files.keepAll")}</button>
        <button type="button" data-testid="undo-all-files" disabled={disabled || !eligible.length} title={t(disabled ? "files.stopBeforeUndo" : !eligible.length ? "files.noneRestorable" : "files.undoHint")}
        onClick={() => setConfirmation({ files: eligible, skipped: rows.length - eligible.length })}>{t("files.undoAll")}</button></>}>
      {sourceTurnId ? <SessionFilesList sessionId={sessionId} sourceTurnId={sourceTurnId} files={rows} busy={disabled} onIntent={onIntent} listRef={listRef}
        onUndo={file => setConfirmation({ files: [file], skipped: 0 })} /> : null}
    </DockSection>
    {files?.error && !rows.length ? <div className="tc-session-dock__error" role="status">{t("files.loadFailed")} <button type="button" onClick={refresh}>{t("common.retry")}</button></div> : null}
    {error ? <div className="tc-session-files__error" role="alert">{error === true ? t("files.updateFailed") : error}</div> : null}
    {confirmation ? <UndoFilesConfirmDialog files={confirmation.files} skipped={confirmation.skipped} busy={disabled} onCancel={() => setConfirmation(null)} onConfirm={confirm} /> : null}
  </>;
}
