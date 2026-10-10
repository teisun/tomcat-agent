import { useT } from "../i18n/LocaleProvider";
import { t as defaultT, type Translator } from "../../../src/shared/i18n";
import type { RefObject } from "react";
import type { SessionFileIntent, SessionFileView } from "../../../src/shared/sessionFiles";

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
export function blockedReason(file: SessionFileView, busy: boolean, t: Translator = defaultT) {
  if (busy) return t("files.stopBeforeUndo");
  if (file.blockedReason === "backup_missing") return t("files.backupMissing");
  if (file.blockedReason === "not_regular_file") return t("files.notRegular");
  return t(file.restorable ? "files.undoThis" : "files.cannotUndo");
}
export function SessionFilesList({ sessionId, sourceTurnId, files, busy, onIntent, listRef, onUndo }: {
  sessionId: string; sourceTurnId: string; files: SessionFileView[]; busy: boolean; onIntent(intent: SessionFileIntent): void; listRef: RefObject<HTMLDivElement | null>; onUndo(file: SessionFileView): void;
}) {
  const t = useT();
  const disabled = busy;
  return <>
    <div className="tc-session-files__list" ref={listRef} data-testid="session-files-list" role="list" aria-label={t("files.changed")}>
      {files.map((file) => <div className="tc-session-files__row" key={file.path} role="listitem" data-testid="session-file-row" data-file-path={file.path}>
        <button type="button" className="tc-session-files__file" data-testid="session-file-diff" title={file.displayPath ?? file.path} onClick={() => onIntent({messageId:crypto.randomUUID(),type:"openSessionFileDiff",data:{sessionId,sourceTurnId,path:file.path}})}>
          <span className={`tc-session-files__name${file.status === "deleted" ? " tc-session-files__name--deleted" : ""}`}>{basename(file.path)}</span>
          {directoryLabel(file,files) ? <span className="tc-session-files__directory">{directoryLabel(file,files)}</span> : null}
          <span className="tc-session-files__stats" title={file.added == null || file.removed == null ? t("files.noCounts") : !file.added && !file.removed ? t("files.onlyEndings") : undefined}>
            {file.added == null || file.removed == null || !file.added && !file.removed ? "—" : <>{file.added ? <span className="tc-session-files__added">+{file.added}</span> : null}{file.removed ? <span className="tc-session-files__removed">-{file.removed}</span> : null}</>}
          </span>
        </button>
        <span className="tc-session-files__undo-slot" title={blockedReason(file,disabled,t)}><button className="tc-session-files__undo" type="button" data-testid="undo-file" aria-label={t("files.undoNamed", { name: basename(file.path) })} disabled={disabled || !file.restorable} onClick={() => onUndo(file)}><span aria-hidden="true" className="codicon codicon-close" /></button></span>
      </div>)}
    </div>
  </>;
}
