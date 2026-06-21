import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
} from "./serveTestUtils";

describe("real tomcat serve scoped session listing", () => {
  it("lists disk-backed project history with isCurrent markers", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const sessionRouter = new SessionRouter(
        runtime.messenger,
        () => runtime.fixture.workspacePath,
      );
      sessionRouter.setBootstrapSessionId(init.sessionId);

      const sessionA = init.sessionId!;
      const sessionB = await sessionRouter.newSession();
      const diskSessions = await sessionRouter.listSessions("disk");

      expect(diskSessions.scope).toBe("disk");
      expect(diskSessions.activeSessionId).toBe(sessionB);
      expect(diskSessions.sessions.map((session) => session.sessionId)).toEqual(
        expect.arrayContaining([sessionA, sessionB]),
      );
      expect(
        diskSessions.sessions.filter((session) => session.isCurrent).map(
          (session) => session.sessionId,
        ),
      ).toEqual([sessionB]);
      expect(
        diskSessions.sessions.every((session) => session.updatedAt !== null),
      ).toBe(true);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
