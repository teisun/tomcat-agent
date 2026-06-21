import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  waitForEvent,
  writePlanFile,
} from "./serveTestUtils";

describe("real tomcat serve plan integration", () => {
  it("supports enter, build, and exit semantics with stable error codes", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("building plan"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);

      const buildBlocked = await runtime.messenger.sendSetPlanMode({
        action: "build",
        sessionId: init.sessionId,
      });
      expect(buildBlocked.success).toBe(false);
      expect(buildBlocked.error).toBe("plan_build_blocked");

      const exitWhileChat = await runtime.messenger.sendSetPlanMode({
        action: "exit",
        sessionId: init.sessionId,
      });
      expect(exitWhileChat.success).toBe(false);
      expect(exitWhileChat.error).toBe("plan_state_conflict");

      const enter = await runtime.messenger.sendSetPlanMode({
        action: "enter",
        sessionId: init.sessionId,
      });
      expect(enter.success).toBe(true);
      expect(enter.payload?.planState).toBe("planning");

      const enterAgain = await runtime.messenger.sendSetPlanMode({
        action: "enter",
        sessionId: init.sessionId,
      });
      expect(enterAgain.success).toBe(false);
      expect(enterAgain.error).toBe("plan_already_in_mode");

      const exit = await runtime.messenger.sendSetPlanMode({
        action: "exit",
        sessionId: init.sessionId,
      });
      expect(exit.success).toBe(true);
      expect(exit.payload?.planState).toBe("chat");

      const planPath = await writePlanFile(
        runtime.fixture.homePath,
        "stage-a-plan-build",
        "planning",
      );
      const planBuild = waitForEvent(
        runtime.messenger,
        (event) => event.type === "plan.build",
      );
      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end",
      );

      const build = await runtime.messenger.sendSetPlanMode({
        action: "build",
        planId: planPath,
        sessionId: init.sessionId,
      });
      const buildEvents = await planBuild;
      const endEvents = await agentEnd;

      expect(build.success).toBe(true);
      expect(build.payload).toMatchObject({
        planId: "stage-a-plan-build",
        planState: "executing",
      });
      expect(String(build.payload?.planPath)).toContain(".plan.md");
      expect(
        buildEvents.some(
          (event) =>
            event.type === "plan.build" &&
            event.planId === "stage-a-plan-build",
        ),
      ).toBe(true);
      expect(endEvents.at(-1)?.type).toBe("agent_end");
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
