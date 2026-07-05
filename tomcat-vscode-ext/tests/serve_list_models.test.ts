import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  warmTomcatBinaryForSuite,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

describe("real tomcat serve model integration", () => {
  it("lists models and persists the selected model in get_state", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const list = await runtime.messenger.sendListModels();

      expect(list.success).toBe(true);
      expect(list.payload?.models).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            baseUrl: server.baseUrl,
            id: "gpt-5.4",
            provider: "openai",
          }),
          expect.objectContaining({
            id: "deepseek-v4-pro",
            provider: "deepseek",
          }),
        ]),
      );

      const setModel = await runtime.messenger.sendSetModel(
        init.sessionId,
        "deepseek-v4-pro",
      );
      expect(setModel.success).toBe(true);

      const state = await runtime.messenger.request({
        sessionId: init.sessionId,
        type: "get_state",
      });
      expect(state.success).toBe(true);
      expect(state.payload?.model).toBe("deepseek-v4-pro");
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
