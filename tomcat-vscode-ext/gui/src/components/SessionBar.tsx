import { useEffect, useMemo, useRef, useState } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, type Translator } from "../../../src/shared/i18n";

import type {
  WebviewConnectionStatus,
  WebviewSessionTab,
} from "../types";
import { groupSessionsByDate, type SessionGroup } from "./sessionList/groupSessions";
import { ConfirmationDialog } from "./ConfirmationDialog";
import type { SessionActionFeedback } from "../../../src/ui/webview/protocol";
import type { MessageKey } from "../../../src/shared/i18n";

const FEEDBACK_LABELS: Record<SessionActionFeedback["code"], MessageKey> = {
  busy: "session.delete.busy", session_in_use: "session.delete.inUse", session_scope_mismatch: "session.delete.scope",
  unknown: "session.delete.unknown", partial_cleanup: "session.delete.partial", fallback_failed: "session.delete.fallback",
  failed: "session.delete.failed", pending_changes: "session.delete.waitForChanges", retained: "session.delete.retained", unavailable: "session.delete.unavailable",
};

const CAP_PER_GROUP = 6;

function formatSessionLabel(session: WebviewSessionTab, t: Translator): string {
  const meta: string[] = [];
  if (session.isCurrent) {
    meta.push("*");
  }
  if (session.busy) {
    meta.push(t("session.running"));
  }
  const suffix = meta.length ? ` (${meta.join(" · ")})` : "";
  const trimmedTitle = session.title?.trim();
  const base = trimmedTitle && trimmedTitle.length > 0 ? trimmedTitle : t("session.new");
  return `${base}${suffix}`;
}

export function SessionBar({
  activeSessionId,
  connectionStatus,
  creating = false,
  creationDisabledReason,
  onOpenSettings,
  onSetPinned,
  onDeleteSession,
  deleteSupported = false,
  actionFeedback,
  onNewSession,
  ready,
  onSwitchSession,
  pendingQuestionCounts = {},
  sessions,
}: {
  activeSessionId: string | null;
  connectionStatus?: WebviewConnectionStatus;
  creating?: boolean;
  creationDisabledReason?: string;
  onOpenSettings(): void;
  onSetPinned?(sessionId: string, pinned: boolean): void;
  onDeleteSession?(sessionId: string): void;
  deleteSupported?: boolean;
  actionFeedback?: SessionActionFeedback | null;
  onNewSession(): void;
  ready: boolean;
  onSwitchSession(sessionId: string): void;
  pendingQuestionCounts?: Record<string, number>;
  sessions: WebviewSessionTab[];
}) {
  const t = useT();
  const locale = useLocale();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const closeForAction = () => { setOpen(false); triggerRef.current?.focus(); };
  const [open, setOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<WebviewSessionTab | null>(null);
  const deleteConfirmed = useRef(false);
  const [dismissedFeedback, setDismissedFeedback] = useState<string | null>(null);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const wrapperRef = useRef<HTMLDivElement>(null);

  const groups = useMemo<SessionGroup[]>(() => groupSessionsByDate(sessions), [sessions]);

  const activeSession = useMemo(
    () => sessions.find((session) => session.sessionId === activeSessionId) ?? null,
    [sessions, activeSessionId],
  );

  useEffect(() => {
    if (creating) setOpen(false);
  }, [creating]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handleClickOutside = (event: MouseEvent) => {
      if (!wrapperRef.current) {
        return;
      }
      if (event.target instanceof Node && !wrapperRef.current.contains(event.target)) {
        setOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [open]);

  const triggerLabel = activeSession
    ? formatSessionLabel(activeSession, t)
    : sessions.length
      ? t("session.choose")
      : t("session.empty");
  const connected =
    connectionStatus === "ready" || (!connectionStatus && ready);
  const connectionLabel = t(connected ? "connection.ready"
    : connectionStatus === "reconnecting" ? "connection.reconnecting"
    : connectionStatus === "degraded" ? "connection.degraded"
    : connectionStatus === "failed" ? "connection.failed" : "connection.connecting");

  const toggleGroup = (label: string) => {
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (next.has(label)) {
        next.delete(label);
      } else {
        next.add(label);
      }
      return next;
    });
  };

  const handlePick = (sessionId: string) => {
    onSwitchSession(sessionId);
    setOpen(false);
  };

  return (
    <><section className="tc-topbar" aria-label={t("session.bar")} ref={wrapperRef}>
      <span
        aria-label={connectionLabel}
        className={`tc-conn-light tc-conn-light--${connected ? "connected" : "connecting"}`}
        data-testid="connection-chip"
        title={connectionLabel}
      />
      <button
        aria-expanded={open}
        aria-label={t("session.select")}
        ref={triggerRef}
        className="tc-topbar__trigger"
        data-testid="session-select"
        disabled={!ready || creating}
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        <span className="tc-topbar__trigger-label">{triggerLabel}</span>
        <span className="tc-topbar__caret" aria-hidden="true">
          {open ? "▴" : "▾"}
        </span>
      </button>
      <button
        aria-busy={creating}
        aria-label={t(creating ? "session.creating" : "session.create")}
        className="tc-icon-button tc-topbar__new"
        data-testid="new-session-button"
        title={creationDisabledReason}
        disabled={!ready || creating || !!creationDisabledReason}
        onClick={onNewSession}
        type="button"
      >
        {creating ? "…" : "+"}
      </button>
      <button
        aria-label={t("settings.title")}
        className="tc-icon-button tc-topbar__settings"
        data-testid="settings-button"
        onClick={onOpenSettings}
        title={t("settings.title")}
        type="button"
      >
        <span aria-hidden="true" className="codicon codicon-settings-gear" />
      </button>
      {open ? (
        <div className="tc-session-dropdown" data-testid="session-dropdown" role="list" aria-label={t("session.list")}>
          {groups.length === 0 ? (
            <div className="tc-session-dropdown__empty">{t("session.empty")}</div>
          ) : (
            groups.map((group) => {
              const isExpanded = expandedGroups.has(group.id);
              const visible = isExpanded
                ? group.sessions
                : group.sessions.slice(0, CAP_PER_GROUP);
              const remaining = group.sessions.length - visible.length;
              return (
                <section className="tc-session-group" key={group.id}>
                  <h3 className="tc-session-group__header" data-testid="session-group-header">
                    {t(group.labelKey)}
                  </h3>
                  {visible.map((session) => {
                    const isActive = session.sessionId === activeSessionId;
                    const pendingCount = isActive
                      ? 0
                      : (pendingQuestionCounts[session.sessionId] ?? 0);
                    const pinLabel = t(session.isPinned ? "session.unpin" : "session.pin");
                    const busy = session.busy || (pendingQuestionCounts[session.sessionId] ?? 0) > 0;
                    const deleteReason = session.deleting ? t("session.delete.pending")
                      : busy ? t("session.delete.busy")
                      : !deleteSupported ? t("session.delete.unavailable") : t("session.delete");
                    return (
                      <div className={`tc-session-row${isActive ? " tc-session-row--active" : ""}`} role="listitem" key={session.sessionId}>
                      <button
                        aria-current={isActive ? "true" : undefined}
                        aria-label={`${formatSessionLabel(session, t)}${
                          pendingCount > 0
                            ? `, ${t(pluralKey(locale, "session.questions.other", pendingCount), { count: pendingCount })}`
                            : ""
                        }`}
                        className="tc-session-item"
                        data-testid="session-option"
                        onClick={() => handlePick(session.sessionId)}
                        title={session.title ?? session.sessionId}
                        type="button"
                      >
                        <span className="tc-session-item__title">
                          {formatSessionLabel(session, t)}
                        </span>
                        {pendingCount > 0 ? (
                          <span
                            aria-hidden="true"
                            className="tc-session-item__pending-badge"
                            data-testid="session-pending-badge"
                          >
                            {pendingCount}
                          </span>
                        ) : null}
                      </button>
                      {onSetPinned ? <button className="tc-icon-button tc-session-row__action" type="button"
                        aria-label={pinLabel} aria-pressed={session.isPinned === true} title={pinLabel}
                        disabled={session.deleting}
                        onClick={() => { closeForAction(); onSetPinned(session.sessionId, !session.isPinned); }}>
                        <span aria-hidden="true" className={`codicon ${session.isPinned ? "codicon-pinned" : "codicon-pin"}`} />
                      </button> : null}
                      {onDeleteSession ? <span title={deleteReason}><button className="tc-icon-button tc-session-row__action" type="button"
                        aria-label={t("session.delete")} title={deleteReason} disabled={!deleteSupported || busy || session.deleting}
                        onClick={() => { closeForAction(); deleteConfirmed.current = false; setDeleteTarget(session); }}>
                        <span aria-hidden="true" className="codicon codicon-trash" />
                      </button></span> : null}
                      </div>
                    );
                  })}
                  {remaining > 0 ? (
                    <button
                      className="tc-session-group__more"
                      data-testid="session-more"
                      onClick={() => toggleGroup(group.id)}
                      type="button"
                    >
                      {t(pluralKey(locale, "session.more.other", remaining), { count: remaining })}
                    </button>
                  ) : null}
                </section>
              );
            })
          )}
        </div>
      ) : null}
    </section>
    {actionFeedback && actionFeedback.id !== dismissedFeedback ? <div className="tc-banner tc-session-action-feedback" role="status">
      <span>{t(FEEDBACK_LABELS[actionFeedback.code], { detail: actionFeedback.detail ?? "" })}</span>
      <button className="tc-icon-button" type="button" aria-label={t("common.close")} title={t("common.close")} onClick={() => setDismissedFeedback(actionFeedback.id)}><span className="codicon codicon-close" aria-hidden="true" /></button>
    </div> : null}
    {deleteTarget ? <ConfirmationDialog testId="delete-session" cancelLabel={t("common.cancel")}
      title={t("session.delete.title", { title: deleteTarget.title?.trim() || t("session.new") })}
      body={t("session.delete.confirm")}
      actions={[{ id: "delete", label: t("session.delete"), tone: "primary", disabled: !ready || !deleteSupported || sessions.some(s => s.sessionId === deleteTarget.sessionId && (s.busy || s.deleting)) }]}
      onCancel={() => setDeleteTarget(null)}
      onAction={(action) => {
        if (action !== "delete" || deleteConfirmed.current) return;
        deleteConfirmed.current = true;
        setDeleteTarget(null);
        onDeleteSession?.(deleteTarget.sessionId);
      }} /> : null}</>
  );
}
