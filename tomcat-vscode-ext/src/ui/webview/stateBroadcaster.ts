import type { WebviewSessionPatchOp, WebviewTimelineItem } from "./protocol";

type TimerHandle = ReturnType<typeof setTimeout>;

export interface StateBroadcasterClock {
  clearTimeout(handle: TimerHandle): void;
  setTimeout(callback: () => void, delayMs: number): TimerHandle;
}

export interface SessionPatchFlush {
  ops: WebviewSessionPatchOp[];
  seq: number;
  sessionId: string;
}

export interface StateBroadcasterFlushPlan {
  fullState: boolean;
  sessionIds: string[];
  sessionPatches: SessionPatchFlush[];
}

export interface StateBroadcasterOptions {
  clock?: StateBroadcasterClock;
  delayMs?: number;
  flush(plan: StateBroadcasterFlushPlan): Promise<void>;
}

function clonePlain<T>(value: T): T {
  if (typeof structuredClone === "function") {
    return structuredClone(value);
  }
  return JSON.parse(JSON.stringify(value)) as T;
}

function cloneTimelineItem(item: WebviewTimelineItem): WebviewTimelineItem {
  return clonePlain(item);
}

function clonePatchOp(op: WebviewSessionPatchOp): WebviewSessionPatchOp {
  if (op.type === "upsert") {
    return {
      ...op,
      item: cloneTimelineItem(op.item),
    };
  }
  return { ...op };
}

function mergeAppendIntoUpsert(
  existing: Extract<WebviewSessionPatchOp, { type: "upsert" }>,
  deltaText: string,
): boolean {
  if (existing.item.type !== "message" && existing.item.type !== "thinking") {
    return false;
  }
  existing.item = {
    ...existing.item,
    text: `${existing.item.text}${deltaText}`,
  };
  return true;
}

function mergePatchOp(
  target: WebviewSessionPatchOp[],
  incoming: WebviewSessionPatchOp,
): void {
  if (incoming.type === "appendText" && incoming.text.length === 0) {
    return;
  }

  if (incoming.type === "appendText") {
    for (let index = target.length - 1; index >= 0; index -= 1) {
      const existing = target[index];
      if (existing.type === "appendText" && existing.id === incoming.id) {
        existing.text = `${existing.text}${incoming.text}`;
        return;
      }
      if (existing.type === "upsert" && existing.item.id === incoming.id) {
        if (mergeAppendIntoUpsert(existing, incoming.text)) {
          return;
        }
        break;
      }
    }
  }

  if (incoming.type === "upsert") {
    for (let index = target.length - 1; index >= 0; index -= 1) {
      const existing = target[index];
      if (existing.type === "appendText" && existing.id === incoming.item.id) {
        target.splice(index, 1);
        continue;
      }
      if (existing.type === "upsert" && existing.item.id === incoming.item.id) {
        target[index] = clonePatchOp(incoming);
        return;
      }
      if (existing.type === "remove" && existing.id === incoming.item.id) {
        target[index] = clonePatchOp(incoming);
        return;
      }
    }
  }

  if (incoming.type === "remove") {
    for (let index = target.length - 1; index >= 0; index -= 1) {
      const existing = target[index];
      if (existing.type === "appendText" && existing.id === incoming.id) {
        target.splice(index, 1);
        continue;
      }
      if (existing.type === "upsert" && existing.item.id === incoming.id) {
        target.splice(index, 1);
        return;
      }
      if (existing.type === "remove" && existing.id === incoming.id) {
        return;
      }
    }
  }

  target.push(clonePatchOp(incoming));
}

type PendingPatchBucket = {
  ops: WebviewSessionPatchOp[];
  seq: number;
};

const DEFAULT_CLOCK: StateBroadcasterClock = {
  clearTimeout,
  setTimeout,
};

export class StateBroadcaster {
  private readonly clock: StateBroadcasterClock;
  private readonly delayMs: number;
  private pendingFullState = false;
  private readonly pendingSessionIds = new Set<string>();
  private readonly pendingSessionPatches = new Map<string, PendingPatchBucket>();
  private readonly patchSeqBySession = new Map<string, number>();
  private flushChain: Promise<void> = Promise.resolve();
  private timer: TimerHandle | null = null;

  constructor(private readonly options: StateBroadcasterOptions) {
    this.clock = options.clock ?? DEFAULT_CLOCK;
    this.delayMs = options.delayMs ?? 16;
  }

  markFullState(): void {
    this.pendingFullState = true;
    this.pendingSessionIds.clear();
    this.pendingSessionPatches.clear();
    this.ensureTimer();
  }

  markSession(sessionId: string): void {
    if (!sessionId) {
      return;
    }
    if (this.pendingFullState) {
      return;
    }
    this.pendingSessionIds.add(sessionId);
    this.pendingSessionPatches.delete(sessionId);
    this.ensureTimer();
  }

  appendPatch(sessionId: string, ops: WebviewSessionPatchOp[]): void {
    if (!sessionId || ops.length === 0) {
      return;
    }
    if (this.pendingFullState || this.pendingSessionIds.has(sessionId)) {
      return;
    }
    const bucket =
      this.pendingSessionPatches.get(sessionId) ??
      {
        ops: [],
        seq: (this.patchSeqBySession.get(sessionId) ?? 0) + 1,
      };
    for (const op of ops) {
      mergePatchOp(bucket.ops, op);
    }
    if (bucket.ops.length === 0) {
      this.pendingSessionPatches.delete(sessionId);
      return;
    }
    this.pendingSessionPatches.set(sessionId, bucket);
    this.ensureTimer();
  }

  async forceFlush(): Promise<void> {
    await this.enqueueFlush();
  }

  dispose(): void {
    this.clearTimer();
    this.pendingFullState = false;
    this.pendingSessionIds.clear();
    this.pendingSessionPatches.clear();
  }

  private clearTimer(): void {
    if (!this.timer) {
      return;
    }
    this.clock.clearTimeout(this.timer);
    this.timer = null;
  }

  private ensureTimer(): void {
    if (this.timer) {
      return;
    }
    this.timer = this.clock.setTimeout(() => {
      void this.enqueueFlush();
    }, this.delayMs);
  }

  private enqueueFlush(): Promise<void> {
    this.flushChain = this.flushChain.then(async () => {
      this.clearTimer();
      const plan = this.takePlan();
      if (!plan) {
        return;
      }
      await this.options.flush(plan);
      for (const patch of plan.sessionPatches) {
        this.patchSeqBySession.set(patch.sessionId, patch.seq);
      }
    });
    return this.flushChain;
  }

  private takePlan(): StateBroadcasterFlushPlan | null {
    if (
      !this.pendingFullState &&
      this.pendingSessionIds.size === 0 &&
      this.pendingSessionPatches.size === 0
    ) {
      return null;
    }

    if (this.pendingFullState) {
      this.pendingFullState = false;
      this.pendingSessionIds.clear();
      this.pendingSessionPatches.clear();
      return {
        fullState: true,
        sessionIds: [],
        sessionPatches: [],
      };
    }

    const sessionIds = [...this.pendingSessionIds].sort();
    const sessionPatches = [...this.pendingSessionPatches.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([sessionId, bucket]) => ({
        ops: bucket.ops.map(clonePatchOp),
        seq: bucket.seq,
        sessionId,
      }));

    this.pendingSessionIds.clear();
    this.pendingSessionPatches.clear();

    return {
      fullState: false,
      sessionIds,
      sessionPatches,
    };
  }
}
