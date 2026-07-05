import type {
  SessionListPayload,
  SessionRouter,
} from "../../serveClient/sessionRouter";

export class TomcatSessionPool {
  constructor(private readonly sessionRouter: SessionRouter) {}

  async createSession(cwd?: string): Promise<string> {
    return this.sessionRouter.newSession(cwd);
  }

  pickDefaultSession(payload: SessionListPayload): string | null {
    return (
      payload.sessions.find((session) => session.isCurrent)?.sessionId ??
      payload.activeSessionId ??
      payload.sessions[0]?.sessionId ??
      null
    );
  }

  async refresh(): Promise<SessionListPayload> {
    return this.sessionRouter.listSessions("disk");
  }

  async release(sessionId: string): Promise<boolean> {
    return this.sessionRouter.closeSession(sessionId);
  }

  async switchTo(sessionId: string): Promise<string> {
    return this.sessionRouter.switchSession(sessionId);
  }
}
