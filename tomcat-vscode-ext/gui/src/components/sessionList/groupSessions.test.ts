import { describe, expect, it } from "vitest";

import type { WebviewSessionTab } from "../../types";
import { groupSessionsByDate } from "./groupSessions";

const MS_PER_DAY = 24 * 60 * 60 * 1000;

function makeSession(sessionId: string, updatedAt: number | null): WebviewSessionTab {
  return {
    busy: false,
    isCurrent: false,
    ownedByThisFrontend: true,
    sessionId,
    title: null,
    updatedAt,
  };
}

describe("groupSessionsByDate", () => {
  it("returns an empty array when there are no sessions", () => {
    expect(groupSessionsByDate([], 0)).toEqual([]);
  });

  it("skips empty buckets and keeps the canonical section order", () => {
    const now = new Date(2026, 5, 25, 14, 0, 0).getTime();
    const sessions = [
      makeSession("today", now - 1000),
      makeSession("last7", now - 5 * MS_PER_DAY),
      makeSession("older", now - 400 * MS_PER_DAY),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups.map((g) => g.label)).toEqual(["Today", "Last 7 days", "Older"]);
  });

  it("puts a session at exactly start-of-today into Today", () => {
    const startOfToday = new Date(2026, 5, 25, 0, 0, 0, 0).getTime();
    const sessions = [makeSession("s", startOfToday)];
    const groups = groupSessionsByDate(sessions, startOfToday + 10 * 60 * 1000);
    expect(groups[0].label).toBe("Today");
    expect(groups[0].sessions.map((s) => s.sessionId)).toEqual(["s"]);
  });

  it("treats 23:59 yesterday as Yesterday, not Today", () => {
    const now = new Date(2026, 5, 25, 0, 30, 0).getTime();
    const lateYesterday = new Date(2026, 5, 24, 23, 59, 59).getTime();
    const sessions = [makeSession("late", lateYesterday)];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups[0].label).toBe("Yesterday");
  });

  it("uses rolling 7-day and 30-day windows for the middle buckets", () => {
    const now = new Date(2026, 5, 25, 12, 0, 0).getTime();
    const sessions = [
      makeSession("d3", now - 3 * MS_PER_DAY),
      makeSession("d10", now - 10 * MS_PER_DAY),
      makeSession("d20", now - 20 * MS_PER_DAY),
      makeSession("d60", now - 60 * MS_PER_DAY),
    ];
    const groups = groupSessionsByDate(sessions, now);
    const byLabel = Object.fromEntries(groups.map((g) => [g.label, g.sessions]));
    expect(byLabel["Last 7 days"].map((s) => s.sessionId)).toEqual(["d3"]);
    expect(byLabel["Last 30 days"].map((s) => s.sessionId)).toEqual(["d10", "d20"]);
    expect(byLabel["Older"].map((s) => s.sessionId)).toEqual(["d60"]);
  });

  it("falls back to Older when updatedAt is null or NaN", () => {
    const now = new Date(2026, 5, 25, 12, 0, 0).getTime();
    const sessions = [makeSession("null", null), makeSession("nan", Number.NaN)];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups[0].label).toBe("Older");
    expect(groups[0].sessions.map((s) => s.sessionId)).toEqual(["null", "nan"]);
  });

  it("preserves the input order inside each bucket", () => {
    const now = new Date(2026, 5, 25, 12, 0, 0).getTime();
    const sessions = [
      makeSession("a", now - 1000),
      makeSession("b", now - 2000),
      makeSession("c", now - 3000),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups[0].sessions.map((s) => s.sessionId)).toEqual(["a", "b", "c"]);
  });
});
