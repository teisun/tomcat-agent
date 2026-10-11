import { describe, expect, it } from "vitest";

import { StateBroadcaster, type StateBroadcasterClock, type StateBroadcasterFlushPlan } from "../stateBroadcaster";

function createClock(): {
  clock: StateBroadcasterClock;
  flush(): void;
} {
  let nextId = 1;
  const timers = new Map<number, () => void>();
  return {
    clock: {
      clearTimeout(handle) {
        timers.delete(handle as unknown as number);
      },
      setTimeout(callback) {
        const id = nextId;
        nextId += 1;
        timers.set(id, callback);
        return id as unknown as ReturnType<typeof setTimeout>;
      },
    },
    flush() {
      const callbacks = [...timers.values()];
      timers.clear();
      for (const callback of callbacks) {
        callback();
      }
    },
  };
}

function messageItem(id: string, text: string) {
  return {
    id,
    kind: "assistant" as const,
    text,
    type: "message" as const,
  };
}

async function settleFlushQueue(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("StateBroadcaster", () => {
  it("coalesces repeated appendText ops into one patch flush", async () => {
    const { clock, flush } = createClock();
    const plans: StateBroadcasterFlushPlan[] = [];
    const broadcaster = new StateBroadcaster({
      clock,
      delayMs: 16,
      flush: async (plan) => {
        plans.push(plan);
      },
    });

    broadcaster.appendPatch("s1", [
      { item: messageItem("m1", "h"), type: "upsert" },
    ]);
    broadcaster.appendPatch("s1", [
      { id: "m1", text: "i", type: "appendText" },
      { id: "m1", text: "!", type: "appendText" },
    ]);

    flush();
    await settleFlushQueue();

    expect(plans).toEqual([
      {
        fullState: false,
        sessionIds: [],
        sessionPatches: [
          {
            ops: [
              {
                item: messageItem("m1", "hi!"),
                type: "upsert",
              },
            ],
            seq: 1,
            sessionId: "s1",
          },
        ],
      },
    ]);
  });

  it("lets a session snapshot override pending patches for the same session", async () => {
    const plans: StateBroadcasterFlushPlan[] = [];
    const broadcaster = new StateBroadcaster({
      flush: async (plan) => {
        plans.push(plan);
      },
    });

    broadcaster.appendPatch("s1", [
      { item: messageItem("m1", "hello"), type: "upsert" },
    ]);
    broadcaster.markSession("s1");
    await broadcaster.forceFlush();

    expect(plans).toEqual([
      {
        fullState: false,
        sessionIds: ["s1"],
        sessionPatches: [],
      },
    ]);
  });

  it("lets a full-state flush override pending session and patch frames", async () => {
    const plans: StateBroadcasterFlushPlan[] = [];
    const broadcaster = new StateBroadcaster({
      flush: async (plan) => {
        plans.push(plan);
      },
    });

    broadcaster.markSession("s1");
    broadcaster.appendPatch("s2", [
      { item: messageItem("m2", "hello"), type: "upsert" },
    ]);
    broadcaster.markFullState();
    await broadcaster.forceFlush();

    expect(plans).toEqual([
      {
        fullState: true,
        sessionIds: [],
        sessionPatches: [],
      },
    ]);
  });

  it("reserves a patch sequence before the in-flight flush finishes", async () => {
    const { clock } = createClock();
    const plans: StateBroadcasterFlushPlan[] = [];
    let release!: () => void;
    let entered!: () => void;
    const barrier = new Promise<void>(resolve => { release = resolve; });
    const started = new Promise<void>(resolve => { entered = resolve; });
    const broadcaster = new StateBroadcaster({
      clock,
      flush: async plan => {
        plans.push(plan);
        if (plans.length === 1) { entered(); await barrier; }
      },
    });
    try {
      broadcaster.appendPatch("s1", [{ item: messageItem("m1", "hello"), type: "upsert" }]);
      const pending = broadcaster.forceFlush();
      await started;
      broadcaster.appendPatch("s1", [{ id: "m1", text: " world", type: "appendText" }]);
      release();
      await pending;
      await broadcaster.forceFlush();
      expect(plans.map(plan => plan.sessionPatches[0].seq)).toEqual([1, 2]);
    } finally {
      release();
      broadcaster.dispose();
    }
  });

  it("reports a failed flush without poisoning subsequent snapshots", async () => {
    const { clock } = createClock();
    let attempts = 0;
    const broadcaster = new StateBroadcaster({
      clock,
      flush: async () => {
        if (++attempts === 1) throw new Error("view changed during send");
      },
    });
    try {
      broadcaster.markFullState();
      await expect(broadcaster.forceFlush()).rejects.toThrow("view changed during send");
      broadcaster.markFullState();
      await expect(broadcaster.forceFlush()).resolves.toBeUndefined();
      expect(attempts).toBe(2);
    } finally {
      broadcaster.dispose();
    }
  });

  it("increments patch seq per flushed session", async () => {
    const plans: StateBroadcasterFlushPlan[] = [];
    const broadcaster = new StateBroadcaster({
      flush: async (plan) => {
        plans.push(plan);
      },
    });

    broadcaster.appendPatch("s1", [
      { item: messageItem("m1", "hello"), type: "upsert" },
    ]);
    await broadcaster.forceFlush();
    broadcaster.appendPatch("s1", [
      { id: "m1", text: " world", type: "appendText" },
    ]);
    await broadcaster.forceFlush();

    expect(plans[0]?.sessionPatches[0]?.seq).toBe(1);
    expect(plans[1]?.sessionPatches[0]?.seq).toBe(2);
  });
});
