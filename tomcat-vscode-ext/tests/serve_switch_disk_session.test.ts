import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  waitForEvent,
  warmTomcatBinaryForSuite,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

describe("real tomcat serve disk session switching", () => {
  it("rehydrates a disk-only session and continues the conversation", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("resumed session"), sseFinish("stop"), sseDone()],
      },
    ]);
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
      const closed = await sessionRouter.closeSession(sessionA);
      expect(closed).toBe(true);

      const liveSessions = await sessionRouter.listSessions();
      expect(liveSessions.activeSessionId).toBe(sessionB);
      expect(liveSessions.sessions.map((session) => session.sessionId)).not.toContain(
        sessionA,
      );

      const switched = await sessionRouter.switchSession(sessionA);
      expect(switched).toBe(sessionA);

      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end" && event.sessionId === sessionA,
      );
      await runtime.messenger.request({
        params: {},
        sessionId: sessionA,
        text: "resume this session",
        type: "prompt",
      });
      const events = await agentEnd;

      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            event.sessionId === sessionA &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "resumed session",
        ),
      ).toBe(true);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
