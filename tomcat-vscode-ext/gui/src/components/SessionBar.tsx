import { useEffect, useMemo, useRef, useState } from "react";

import type { WebviewSessionTab } from "../types";
import { groupSessionsByDate, type SessionGroup } from "./sessionList/groupSessions";

const CAP_PER_GROUP = 6;

function formatSessionLabel(session: WebviewSessionTab): string {
  const meta: string[] = [];
  if (session.isCurrent) {
    meta.push("*");
  }
  if (session.busy) {
    meta.push("running");
  }
  const suffix = meta.length ? ` (${meta.join(" · ")})` : "";
  const trimmedTitle = session.title?.trim();
  const base = trimmedTitle && trimmedTitle.length > 0 ? trimmedTitle : "New session";
  return `${base}${suffix}`;
}

export function SessionBar({
  activeSessionId,
  onNewSession,
  ready,
  onSwitchSession,
  sessions,
}: {
  activeSessionId: string | null;
  onNewSession(): void;
  ready: boolean;
  onSwitchSession(sessionId: string): void;
  sessions: WebviewSessionTab[];
}) {
  const [open, setOpen] = useState(false);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const wrapperRef = useRef<HTMLDivElement>(null);

  const groups = useMemo<SessionGroup[]>(() => groupSessionsByDate(sessions), [sessions]);

  const activeSession = useMemo(
    () => sessions.find((session) => session.sessionId === activeSessionId) ?? null,
    [sessions, activeSessionId],
  );

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
    ? formatSessionLabel(activeSession)
    : sessions.length
      ? "Select session"
      : "No sessions";

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
    <section className="tc-topbar" aria-label="Session bar" ref={wrapperRef}>
      <span
        aria-label={ready ? "Connected" : "Connecting…"}
        className={`tc-conn-light tc-conn-light--${ready ? "connected" : "connecting"}`}
        data-testid="connection-chip"
        title={ready ? "Connected" : "Connecting…"}
      />
      <button
        aria-expanded={open}
        aria-label="Tomcat session"
        className="tc-topbar__trigger"
        data-testid="session-select"
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        <span className="tc-topbar__trigger-label">{triggerLabel}</span>
        <span className="tc-topbar__caret" aria-hidden="true">
          {open ? "▴" : "▾"}
        </span>
      </button>
      <button
        aria-label="Create new session"
        className="tc-icon-button tc-topbar__new"
        data-testid="new-session-button"
        onClick={onNewSession}
        type="button"
      >
        +
      </button>
      {open ? (
        <div className="tc-session-dropdown" data-testid="session-dropdown" role="listbox">
          {groups.length === 0 ? (
            <div className="tc-session-dropdown__empty">No sessions</div>
          ) : (
            groups.map((group) => {
              const isExpanded = expandedGroups.has(group.label);
              const visible = isExpanded
                ? group.sessions
                : group.sessions.slice(0, CAP_PER_GROUP);
              const remaining = group.sessions.length - visible.length;
              return (
                <section className="tc-session-group" key={group.label}>
                  <h3 className="tc-session-group__header" data-testid="session-group-header">
                    {group.label}
                  </h3>
                  {visible.map((session) => {
                    const isActive = session.sessionId === activeSessionId;
                    return (
                      <button
                        aria-current={isActive ? "true" : undefined}
                        className={`tc-session-item${isActive ? " tc-session-item--active" : ""}`}
                        data-testid="session-option"
                        key={session.sessionId}
                        onClick={() => handlePick(session.sessionId)}
                        title={session.title ?? session.sessionId}
                        type="button"
                      >
                        <span className="tc-session-item__title">
                          {formatSessionLabel(session)}
                        </span>
                      </button>
                    );
                  })}
                  {remaining > 0 ? (
                    <button
                      className="tc-session-group__more"
                      data-testid="session-more"
                      onClick={() => toggleGroup(group.label)}
                      type="button"
                    >
                      Show {remaining} more
                    </button>
                  ) : null}
                </section>
              );
            })
          )}
        </div>
      ) : null}
    </section>
  );
}
