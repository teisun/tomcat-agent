import { afterEach, describe, expect, it, vi } from "vitest";

import { ComposerWorkRegistry } from "./composerWorkRegistry";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((next) => { resolve = next; });
  return { promise, resolve };
}

describe("ComposerWorkRegistry", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("waits only for source-session work at or before the click cutoff", async () => {
    const registry = new ComposerWorkRegistry();
    const before = registry.begin("source", "paste");
    registry.begin("other", "picker");
    const cutoff = registry.cutoff("source");
    const after = registry.begin("source", "drop");
    const finished = deferred();
    void registry.waitForCutoff("source", cutoff).then(finished.resolve);

    registry.complete(after.operationId);
    await Promise.resolve();
    let didFinish = false;
    finished.promise.then(() => { didFinish = true; });
    await Promise.resolve();
    expect(didFinish).toBe(false);

    registry.complete(before.operationId);
    await finished.promise;
    expect(registry.pendingCount("source")).toBe(0);
    expect(registry.pendingCount("other")).toBe(1);
  });

  it("ignores duplicate and late completions", () => {
    const registry = new ComposerWorkRegistry();
    const ticket = registry.begin("source", "attach");
    expect(registry.complete(ticket.operationId)).toBe(true);
    expect(registry.complete(ticket.operationId)).toBe(false);
    expect(registry.complete("unknown")).toBe(false);
  });

  it("releases a draft fork cutoff after the bounded wait and reports it", async () => {
    vi.useFakeTimers();
    const reportTimeout = vi.fn();
    const registry = new ComposerWorkRegistry(10_000, reportTimeout);
    registry.begin("source", "picker");

    const waiting = registry.waitForCutoff("source", registry.cutoff("source"));
    await vi.advanceTimersByTimeAsync(9_999);
    let settled = false;
    void waiting.then(() => { settled = true; });
    await Promise.resolve();
    expect(settled).toBe(false);

    await vi.advanceTimersByTimeAsync(1);
    await waiting;

    expect(reportTimeout).toHaveBeenCalledWith(
      expect.stringContaining("timed out waiting 10000ms"),
    );
    expect(registry.pendingCount("source")).toBe(1);
  });
});
