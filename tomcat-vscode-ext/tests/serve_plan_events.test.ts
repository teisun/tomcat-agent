import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import type { ServeEvent } from "../src/serveClient/wire";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  buildAndInterruptPlan,
  warmTomcatBinaryForSuite,
  writePlanFile,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

describe("real tomcat serve plan event forwarding", () => {
  it("forwards plan events through the stdio event pump", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("event build"), { ...sseFinish("stop"), delayMs: 1000 }, sseDone()],
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
      const { build, events } = await buildAndInterruptPlan(runtime.messenger, init.sessionId, planPath);
      expect(build.success).toBe(true);
      expect(events.find((event) => event.type === "agent_end")).toBeDefined();
      expect(events.at(-1)?.type).toBe("agent_idle");
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
