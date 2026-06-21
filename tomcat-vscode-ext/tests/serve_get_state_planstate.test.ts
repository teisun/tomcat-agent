import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  writePlanFile,
} from "./serveTestUtils";

describe("real tomcat serve state integration", () => {
  it("reflects planState, planId, and sessionKey through get_state", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("state build"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const initialState = await runtime.messenger.request({
        sessionId: init.sessionId,
        type: "get_state",
      });

      expect(initialState.success).toBe(true);
      expect(initialState.payload).toMatchObject({
        mode: "code",
        model: "gpt-5.4",
        planId: null,
        planState: "chat",
        sessionId: init.sessionId,
      });
      expect(typeof initialState.payload?.sessionKey).toBe("string");

      await runtime.messenger.sendSetPlanMode({
        action: "enter",
        sessionId: init.sessionId,
      });
      const planningState = await runtime.messenger.request({
        sessionId: init.sessionId,
        type: "get_state",
      });
      expect(planningState.payload).toMatchObject({
        planId: null,
        planState: "planning",
        sessionId: init.sessionId,
        sessionKey: initialState.payload?.sessionKey,
      });

      const planPath = await writePlanFile(
        runtime.fixture.homePath,
        "stage-a-state-plan",
        "planning",
      );
      await runtime.messenger.sendSetPlanMode({
        action: "build",
        planId: planPath,
        sessionId: init.sessionId,
      });
      const executingState = await runtime.messenger.request({
        sessionId: init.sessionId,
        type: "get_state",
      });

      expect(executingState.payload).toMatchObject({
        planId: "stage-a-state-plan",
        planState: "executing",
        sessionId: init.sessionId,
        sessionKey: initialState.payload?.sessionKey,
      });
      expect(String(executingState.payload?.planPath)).toContain(
        "stage-a-state-plan.plan.md",
      );
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
