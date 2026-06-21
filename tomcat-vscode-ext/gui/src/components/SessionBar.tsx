import type { WebviewSessionTab } from "../types";

function formatSessionLabel(session: WebviewSessionTab): string {
  const meta: string[] = [];
  if (session.isCurrent) {
    meta.push("*");
  }
  if (session.owner) {
    meta.push(session.owner);
  }
  if (session.busy) {
    meta.push("running");
  }
  const suffix = meta.length ? ` (${meta.join(" · ")})` : "";
  return `${session.sessionId}${suffix}`;
}

export function SessionBar({
  activeSessionId,
  onCloseSession,
  onNewSession,
  onRefreshSessions,
  onSwitchSession,
  sessions,
}: {
  activeSessionId: string | null;
  onCloseSession(): void;
  onNewSession(): void;
  onRefreshSessions(): void;
  onSwitchSession(sessionId: string): void;
  sessions: WebviewSessionTab[];
}) {
  return (
    <section className="tc-sessionbar" aria-label="Session bar">
      <label className="tc-field tc-field--compact tc-sessionbar__field">
        <span>Session</span>
        <select
          aria-label="Tomcat session"
          data-testid="session-select"
          onChange={(event) => {
            if (event.target.value) {
              onSwitchSession(event.target.value);
            }
          }}
          value={activeSessionId ?? ""}
        >
          {sessions.length ? null : <option value="">No sessions</option>}
          {sessions.map((session) => (
            <option
              data-testid="session-option"
              key={session.sessionId}
              title={session.sessionId}
              value={session.sessionId}
            >
              {formatSessionLabel(session)}
            </option>
          ))}
        </select>
      </label>

      <div className="tc-sessionbar__actions">
        <button
          className="tc-button tc-button--secondary"
          onClick={onNewSession}
          type="button"
        >
          New
        </button>
        <button
          className="tc-button tc-button--ghost"
          onClick={onRefreshSessions}
          type="button"
        >
          Refresh
        </button>
        <button
          aria-label="Close active session"
          className="tc-button tc-button--ghost"
          disabled={!activeSessionId}
          onClick={onCloseSession}
          type="button"
        >
          Close
        </button>
      </div>
    </section>
  );
}
