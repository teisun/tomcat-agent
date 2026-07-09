import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
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

describe("real tomcat serve happy path", () => {
  it("streams a prompt roundtrip over stdio", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("hello from serve"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      expect(init.sessionId).toBeTruthy();

      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end",
      );
      const response = await runtime.messenger.request({
        params: {},
        sessionId: init.sessionId,
        text: "say hello",
        type: "prompt",
      });
      const events = await agentEnd;

      expect(response.success).toBe(true);
      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "hello from serve",
        ),
      ).toBe(true);
      expect(events.at(-1)?.type).toBe("agent_end");
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
