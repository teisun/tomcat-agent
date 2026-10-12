import { describe, expect, it, vi } from "vitest";
import { SessionPins } from "../sessionPins";

function fixture() {
  const values = new Map<string, unknown>();
  const storage = {
    get: <T>(key: string, fallback?: T): T => (values.has(key) ? values.get(key) : fallback) as T,
    update: vi.fn(async (key: string, value: unknown) => { if (value === undefined) values.delete(key); else values.set(key, value); }),
  };
  return { storage, pins: new SessionPins(storage) };
}

describe("SessionPins", () => {
  it("persists across reconstruction without changing session identity or current selection", async () => {
    const { storage, pins } = fixture();
    await pins.setPinned("s", true);
    await pins.setPinned("s", true);
    expect(storage.update).toHaveBeenCalledTimes(2);
    const session = { sessionId: "s", updatedAt: 10, isCurrent: false };
    expect(new SessionPins(storage).project(session)).toEqual({ ...session, isPinned: true });
    expect(session).not.toHaveProperty("isPinned");
    await pins.setPinned("s", false);
    expect(pins.isPinned("s")).toBe(false);
  });
  it("keeps missing-list preferences and never projects an orphan as a new session", async () => {
    const { pins } = fixture();
    await pins.setPinned("temporarily-missing", true);
    expect([].map(s => pins.project(s))).toEqual([]);
    expect(pins.project({ sessionId: "another" }).isPinned).toBe(false);
    expect(pins.isPinned("temporarily-missing")).toBe(true);
  });
});
