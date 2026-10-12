import { describe, expect, it } from "vitest";
import { HostDraftCoordinator } from "../hostDraftCoordinator";

describe("HostDraftCoordinator retirement", () => {
  it("waits for the active writer and rejects queued/late work for only the deleted session", async () => {
    const coordinator = new HostDraftCoordinator();
    let release!: () => void;
    let started!: () => void;
    const active = new Promise<void>(resolve => { started = resolve; });
    const hold = new Promise<void>(resolve => { release = resolve; });
    const first = coordinator.run("a", async () => { started(); await hold; return "done"; });
    await active;
    const queued = coordinator.run("a", async () => "must not write");
    const rejected = expect(queued).rejects.toThrow("session_deleted");
    const retired = coordinator.retire("a");
    await expect(coordinator.run("a", async () => "late")).rejects.toThrow("session_deleted");
    expect(await coordinator.run("b", async () => "other session")).toBe("other session");
    release();
    expect(await first).toBe("done");
    await retired; await rejected;
    expect(coordinator.isPending("a")).toBe(false);
    expect(coordinator.isRetired("a")).toBe(true);
  });
});
