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
  return (
    <section className="tc-topbar" aria-label="Session bar">
      <button
        aria-label="Create new session"
        className="tc-icon-button tc-topbar__new"
        data-testid="new-session-button"
        onClick={onNewSession}
        type="button"
      >
        +
      </button>
      <label className="tc-field tc-field--compact tc-topbar__field">
        <span>Session</span>
        <select
          aria-label="Tomcat session"
          className="tc-topbar__select"
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
      <span
        className={ready ? "tc-chip tc-chip--success" : "tc-chip tc-chip--warning"}
        data-testid="connection-chip"
      >
        {ready ? "Connected" : "Connecting..."}
      </span>
    </section>
  );
}
