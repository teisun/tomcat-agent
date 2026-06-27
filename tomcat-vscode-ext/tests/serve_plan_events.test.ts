import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import type { ServeEvent } from "../src/serveClient/wire";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  waitForEvent,
  warmTomcatBinaryForSuite,
  writePlanFile,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

describe("real tomcat serve plan event forwarding", () => {
  it("forwards plan events through the stdio event pump", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("event build"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const planPath = await writePlanFile(
        runtime.fixture.homePath,
        "stage-a-plan-event",
        "planning",
      );
      const planBuild = waitForEvent(
        runtime.messenger,
        (event) =>
          event.type === "plan.build" &&
          event.sessionId === init.sessionId,
      );
      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) =>
          event.type === "agent_end" &&
          event.sessionId === init.sessionId,
      );

      await runtime.messenger.sendSetPlanMode({
        action: "build",
        planId: planPath,
        sessionId: init.sessionId,
      });
      const events = await planBuild;
      await agentEnd;
      const buildEvent = events.find(
        (event): event is Extract<ServeEvent, { type: "plan.build" }> =>
          event.type === "plan.build" && event.sessionId === init.sessionId,
      );

      expect(buildEvent).toMatchObject({
        planId: "stage-a-plan-event",
        sessionId: init.sessionId,
        state: "executing",
        type: "plan.build",
      });
      expect(String(buildEvent?.path)).toContain("stage-a-plan-event.plan.md");
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
