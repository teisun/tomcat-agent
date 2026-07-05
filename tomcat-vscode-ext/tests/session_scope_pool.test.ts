import { describe, expect, it } from "vitest";

import { TomcatSessionPool } from "../src/ui/webview/sessionPool";

describe("webview session scope pool", () => {
  it("requests disk-scoped sessions and picks the current session by default", async () => {
    const requestedScopes: string[] = [];
    const sessionPool = new TomcatSessionPool({
      async closeSession() {
        return true;
      },
      async listSessions(scope = "live") {
        requestedScopes.push(scope);
        return {
          activeSessionId: "s2",
          scope,
          sessions: [
            { busy: false, isCurrent: false, sessionId: "s1", updatedAt: 1 },
            { busy: false, isCurrent: true, sessionId: "s2", updatedAt: 2 },
          ],
        };
      },
      async newSession() {
        return "s3";
      },
      async switchSession(sessionId: string) {
        return sessionId;
      },
    } as never);

    const sessions = await sessionPool.refresh();

    expect(requestedScopes).toEqual(["disk"]);
    expect(sessionPool.pickDefaultSession(sessions)).toBe("s2");
  });
});
