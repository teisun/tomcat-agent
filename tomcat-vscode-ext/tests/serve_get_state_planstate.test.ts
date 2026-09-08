import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
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

describe("real tomcat serve state integration", () => {
  it("returns workspaceMode, agentMode and activePlan without changing session identity", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("state build"), { ...sseFinish("stop"), delayMs: 1000 }, sseDone()] },
    ]);
    let runtime: Awaited<ReturnType<typeof createRealServeMessenger>> | undefined;
    try {
      runtime = await createRealServeMessenger(server.baseUrl);
      const { messenger } = runtime;
      const init = await initializeServe(messenger);
      const getState = () => messenger.request({ sessionId: init.sessionId, type: "get_state" });
      const initial = await getState();
      expect(initial).toMatchObject({ success: true, payload: {
        workspaceMode: "code", model: "gpt-5.4", agentMode: "chat", activePlan: null,
        sessionId: init.sessionId,
      } });
      expect(initial.payload?.sessionKey).toEqual(expect.any(String));
      const identity = { sessionId: init.sessionId, sessionKey: initial.payload?.sessionKey };

      expect(await messenger.sendSetPlanMode({ action: "enter", sessionId: init.sessionId }))
        .toMatchObject({ success: true });
      expect(await getState()).toMatchObject({ success: true, payload: {
        ...identity, workspaceMode: "code", agentMode: "plan", activePlan: null,
      } });

      const planPath = await writePlanFile(runtime.fixture.homePath, "stage-a-state-plan", "planning");
      const { build: built, events, runningState: executing } = await buildAndInterruptPlan(messenger, init.sessionId, planPath);
      expect(built).toMatchObject({ success: true, payload: {
        ...identity, agentMode: "chat", activePlan: { id: "stage-a-state-plan", state: "executing" },
      } });
      expect(executing).toMatchObject({ success: true, payload: {
        ...identity, workspaceMode: "code", agentMode: "chat", activePlan: {
          id: "stage-a-state-plan", state: "executing", path: expect.stringContaining("stage-a-state-plan.plan.md"),
        },
      } });
      expect(events.at(-1)?.type).toBe("agent_idle");
      // Interrupt returns the active plan to pending without replacing identity.
      expect(await getState()).toMatchObject({ success: true, payload: {
        ...identity, workspaceMode: "code", agentMode: "chat",
        activePlan: { id: "stage-a-state-plan", state: "pending" },
      } });
    } finally {
      try { await runtime?.cleanup(); } finally { await server.close(); }
    }
  }, 30_000);
});
