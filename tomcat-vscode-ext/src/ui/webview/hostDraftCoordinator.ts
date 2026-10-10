import { AsyncLocalStorage } from "node:async_hooks";

/**
 * One serialization lane per source session. Draft-affecting host work enters the lane;
 * a fork fence is just another lane item, so it observes every earlier mutation and
 * excludes every later mutation without globally blocking unrelated sessions.
 */
export class HostDraftCoordinator {
  private readonly retired = new Set<string>();

  isRetired(sessionId: string): boolean { return this.retired.has(sessionId); }

  /** Close admission before waiting for work already in the lane; never reopen a deleted ID. */
  async retire(sessionId: string): Promise<void> {
    this.retired.add(sessionId);
    await this.tails.get(sessionId);
  }
  private readonly activeSessionIds = new AsyncLocalStorage<ReadonlySet<string>>();
  private readonly tails = new Map<string, Promise<void>>();

  run<T>(sessionId: string, work: () => Promise<T>): Promise<T> {
    if (this.retired.has(sessionId)) return Promise.reject(new Error("session_deleted"));
    const activeSessionIds = this.activeSessionIds.getStore();
    if (activeSessionIds?.has(sessionId)) {
      throw new Error(
        `HostDraftCoordinator cannot re-enter the active draft lane for session ${sessionId}`,
      );
    }
    const previous = this.tails.get(sessionId) ?? Promise.resolve();
    const current = previous
      .catch(() => undefined)
      .then(() => {
        if (this.retired.has(sessionId)) throw new Error("session_deleted");
        return this.activeSessionIds.run(
          new Set([...(activeSessionIds ?? []), sessionId]), work,
        );
      });
    const tail = current.then(() => undefined, () => undefined);
    this.tails.set(sessionId, tail);
    void tail.finally(() => {
      if (this.tails.get(sessionId) === tail) this.tails.delete(sessionId);
    });
    return current;
  }

  fence(sessionId: string): Promise<void> {
    return this.run(sessionId, async () => undefined);
  }

  isPending(sessionId: string): boolean {
    return this.tails.has(sessionId);
  }
}
