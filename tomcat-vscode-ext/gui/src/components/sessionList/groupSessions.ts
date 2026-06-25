import type { WebviewSessionTab } from "../../types";

export type SessionGroupLabel =
  | "Today"
  | "Yesterday"
  | "Last 7 days"
  | "Last 30 days"
  | "Older";

export interface SessionGroup {
  label: SessionGroupLabel;
  sessions: WebviewSessionTab[];
}

const MS_PER_DAY = 24 * 60 * 60 * 1000;

/**
 * 把会话按 updatedAt 分入时间桶：Today / Yesterday / Last 7 days / Last 30 days / Older。
 * - Today / Yesterday 用本地日历日 0:00 作边界（符合用户对「今天」的直觉）。
 * - Last 7 days / Last 30 days 用 rolling 窗口（now - 7d / now - 30d）。
 * - updatedAt 为 null 的会话归 Older（无法判定时间，放最后）。
 * 输入需已按 updatedAt 倒序（serve 保证）；本函数保持桶内顺序、跳过空桶。
 */
export function groupSessionsByDate(
  sessions: WebviewSessionTab[],
  now: number = Date.now(),
): SessionGroup[] {
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const startOfTodayMs = startOfToday.getTime();
  const startOfYesterdayMs = startOfTodayMs - MS_PER_DAY;
  const last7Threshold = now - 7 * MS_PER_DAY;
  const last30Threshold = now - 30 * MS_PER_DAY;

  const buckets: Record<SessionGroupLabel, WebviewSessionTab[]> = {
    Today: [],
    Yesterday: [],
    "Last 7 days": [],
    "Last 30 days": [],
    Older: [],
  };

  for (const session of sessions) {
    const ts = session.updatedAt;
    if (ts === null || Number.isNaN(ts)) {
      buckets.Older.push(session);
      continue;
    }
    if (ts >= startOfTodayMs) {
      buckets.Today.push(session);
    } else if (ts >= startOfYesterdayMs) {
      buckets.Yesterday.push(session);
    } else if (ts >= last7Threshold) {
      buckets["Last 7 days"].push(session);
    } else if (ts >= last30Threshold) {
      buckets["Last 30 days"].push(session);
    } else {
      buckets.Older.push(session);
    }
  }

  const order: SessionGroupLabel[] = [
    "Today",
    "Yesterday",
    "Last 7 days",
    "Last 30 days",
    "Older",
  ];
  return order
    .map((label) => ({ label, sessions: buckets[label] }))
    .filter((group) => group.sessions.length > 0);
}
