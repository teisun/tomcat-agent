import type { Memento } from "vscode";

const PREFIX = "tomcat.session.pinned.";

/** Workspace-scoped display preference. Disk session metadata remains owned by Serve. */
export class SessionPins {
  constructor(private readonly storage: Pick<Memento, "get" | "update">) {}
  isPinned(sessionId: string): boolean {
    return this.storage.get<boolean>(`${PREFIX}${sessionId}`) === true;
  }
  async setPinned(sessionId: string, pinned: boolean): Promise<void> {
    await this.storage.update(`${PREFIX}${sessionId}`, pinned ? true : undefined);
  }
  project<T extends { sessionId: string }>(session: T): T & { isPinned: boolean } {
    return { ...session, isPinned: this.isPinned(session.sessionId) };
  }
}
