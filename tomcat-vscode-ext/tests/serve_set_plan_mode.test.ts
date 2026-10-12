import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  buildAndInterruptPlan,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
  waitForEvent,
  warmTomcatBinaryForSuite,
  writePlanFile,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

describe("real tomcat serve plan integration", () => {
  it("separates agent mode from the plan file and preserves stable error codes", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("building plan"), { ...sseFinish("stop"), delayMs: 1000 }, sseDone()] },
    ]);
    let runtime: Awaited<ReturnType<typeof createRealServeMessenger>> | undefined;
    try {
      runtime = await createRealServeMessenger(server.baseUrl);
      const { messenger } = runtime;
      const init = await initializeServe(messenger);
      const initial = await messenger.request({ type: "get_state", sessionId: init.sessionId });
      expect(initial.success).toBe(true);
      expect(initial.payload).toMatchObject({ agentMode: "chat", activePlan: null, workspaceMode: "code" });
      const identity = { sessionId: init.sessionId, sessionKey: initial.payload?.sessionKey };
      expect(identity.sessionKey).toEqual(expect.any(String));

      expect(await messenger.sendSetPlanMode({ action: "build", sessionId: init.sessionId }))
        .toMatchObject({ success: false, error: "plan_build_blocked" });
      expect(await messenger.sendSetPlanMode({ action: "exit", sessionId: init.sessionId }))
        .toMatchObject({ success: false, error: "plan_state_conflict" });

      const entered = waitForEvent(messenger, (event) =>
        event.type === "session.agent_mode.changed" && event.agentMode === "plan");
      expect(await messenger.sendSetPlanMode({ action: "enter", sessionId: init.sessionId }))
        .toMatchObject({ success: true, payload: { ...identity, agentMode: "plan", activePlan: null } });
      expect((await entered).at(-1)).toMatchObject({ sessionId: init.sessionId, agentMode: "plan" });
      expect(await messenger.sendSetPlanMode({ action: "enter", sessionId: init.sessionId }))
        .toMatchObject({ success: false, error: "plan_already_in_mode" });
      expect(await messenger.sendSetPlanMode({ action: "exit", sessionId: init.sessionId }))
        .toMatchObject({ success: true, payload: { ...identity, agentMode: "chat", activePlan: null } });

      const planPath = await writePlanFile(runtime.fixture.homePath, "stage-a-plan-build", "planning");
      const { build, events } = await buildAndInterruptPlan(messenger, init.sessionId, planPath);
      expect(build).toMatchObject({
        success: true,
        payload: { ...identity, agentMode: "chat", activePlan: { id: "stage-a-plan-build", state: "executing" } },
      });
      expect(build.payload?.activePlan).toMatchObject({ path: expect.stringContaining("stage-a-plan-build.plan.md") });
      expect(events.find((event) => event.type === "plan.build")).toMatchObject({
        type: "plan.build", planId: "stage-a-plan-build", sessionId: init.sessionId,
      });
      expect(events.find((event) => event.type === "agent_end")).toBeDefined();
      expect(events.at(-1)?.type).toBe("agent_idle");

      // Build already leaves planning mode. Exit cannot complete or discard the file.
      expect(await messenger.sendSetPlanMode({ action: "exit", sessionId: init.sessionId }))
        .toMatchObject({ success: false, error: "plan_state_conflict" });
      const after = await messenger.request({ type: "get_state", sessionId: init.sessionId });
      expect(after).toMatchObject({ success: true, payload: { ...identity, agentMode: "chat", activePlan: { id: "stage-a-plan-build" } } });
      expect(await readFile(planPath, "utf8")).toContain("plan_id: stage-a-plan-build");
    } finally {
      try { await runtime?.cleanup(); } finally { await server.close(); }
    }
  }, 30_000);
});
