import type { WebviewSessionTab } from "../../types";
import type { MessageKey } from "../../../../src/shared/i18n";

export type SessionGroupId = "pinned" | "today" | "yesterday" | "week" | "month" | "older";
export interface SessionGroup {
  id: SessionGroupId;
  labelKey: MessageKey;
  sessions: WebviewSessionTab[];
}
const MS_PER_DAY = 24 * 60 * 60 * 1000;
const labels: Record<SessionGroupId, MessageKey> = {
  pinned: "session.pinned", today: "session.date.today", yesterday: "session.date.yesterday",
  week: "session.date.week", month: "session.date.month", older: "session.date.older",
};

/** Stable bucket IDs keep expansion state independent of the display language. */
export function groupSessionsByDate(sessions: WebviewSessionTab[], now = Date.now()): SessionGroup[] {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const todayMs = today.getTime();
  const yesterdayMs = todayMs - MS_PER_DAY;
  const buckets: Record<SessionGroupId, WebviewSessionTab[]> = { pinned: [], today: [], yesterday: [], week: [], month: [], older: [] };
  for (const session of sessions) {
    const ts = session.updatedAt;
    const id: SessionGroupId = session.isPinned ? "pinned"
      : ts === null || Number.isNaN(ts) ? "older"
      : ts >= todayMs ? "today" : ts >= yesterdayMs ? "yesterday"
      : ts >= now - 7 * MS_PER_DAY ? "week" : ts >= now - 30 * MS_PER_DAY ? "month" : "older";
    buckets[id].push(session);
  }
  const timestamp = (s: WebviewSessionTab) => s.updatedAt !== null && Number.isFinite(s.updatedAt) ? s.updatedAt : -Infinity;
  buckets.pinned.sort((a, b) => timestamp(b) - timestamp(a) || b.sessionId.localeCompare(a.sessionId));
  return (Object.keys(labels) as SessionGroupId[]).map(id => ({ id, labelKey: labels[id], sessions: buckets[id] })).filter(group => group.sessions.length > 0);
}
