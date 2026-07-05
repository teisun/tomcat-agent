import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  responsesCompleted,
  responsesFunctionCallAdded,
  responsesFunctionCallArgumentsDelta,
  responsesTextDelta,
  spawnScriptedOpenAiStreamServer,
  waitForEvent,
  warmTomcatBinaryForSuite,
} from "./serveTestUtils";

warmTomcatBinaryForSuite();

const ASK_QUESTION_ARGS = JSON.stringify({
  questions: [
    {
      id: "q1",
      options: [
        { id: "a", label: "A", recommended: true },
        { id: "b", label: "B", recommended: false },
      ],
      prompt: "Pick one",
    },
  ],
});

const MULTI_QUESTION_ARGS = JSON.stringify({
  questions: [
    {
      id: "q1",
      options: [
        { id: "day", label: "Day", recommended: true },
        { id: "night", label: "Night", recommended: false },
      ],
      prompt: "When do you prefer to code?",
    },
    {
      id: "q2",
      options: [
        { id: "ts", label: "TypeScript", recommended: true },
        { id: "rs", label: "Rust", recommended: false },
      ],
      prompt: "Which language do you want to use?",
    },
  ],
});

describe("real tomcat serve ask_question integration", () => {
  it("answers control_request and resumes the turn", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [
          responsesFunctionCallAdded("fc_1", "call_1", "ask_question"),
          responsesFunctionCallArgumentsDelta("fc_1", ASK_QUESTION_ARGS),
          responsesCompleted(),
        ],
      },
      {
        parts: [responsesTextDelta("after approval"), responsesCompleted()],
      },
    ]);
    const runtime = await createRealServeMessenger(
      server.baseUrl,
      "openai-responses",
    );

    try {
      let sawAskQuestion = false;
      runtime.messenger.registerAskQuestionHandler(async () => {
        sawAskQuestion = true;
        return {
          answers: [
            {
              customText: null,
              optionIds: ["a"],
              pickedRecommended: true,
              questionId: "q1",
              skipped: false,
            },
          ],
          cancelled: false,
        };
      });

      const init = await initializeServe(runtime.messenger);
      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end",
      );

      await runtime.messenger.request({
        params: {},
        sessionId: init.sessionId,
        text: "ask me a question",
        type: "prompt",
      });
      const events = await agentEnd;

      expect(sawAskQuestion).toBe(true);
      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "after approval",
        ),
      ).toBe(true);
      expect(server.capturedRequests()).toHaveLength(2);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);

  it("accepts multi-question answers and resumes the turn without ToolError", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [
          responsesFunctionCallAdded("fc_2", "call_2", "ask_question"),
          responsesFunctionCallArgumentsDelta("fc_2", MULTI_QUESTION_ARGS),
          responsesCompleted(),
        ],
      },
      {
        parts: [responsesTextDelta("multi approval complete"), responsesCompleted()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");

    try {
      let sawAskQuestion = false;
      runtime.messenger.registerAskQuestionHandler(async () => {
        sawAskQuestion = true;
        return {
          answers: [
            {
              optionIds: ["day"],
              pickedRecommended: true,
              questionId: "q1",
            },
            {
              customText: "Rust",
              optionIds: ["__custom__"],
              pickedRecommended: false,
              questionId: "q2",
            },
          ],
          cancelled: false,
        };
      });

      const init = await initializeServe(runtime.messenger);
      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end",
      );

      await runtime.messenger.request({
        params: {},
        sessionId: init.sessionId,
        text: "ask me two questions",
        type: "prompt",
      });
      const events = await agentEnd;

      expect(sawAskQuestion).toBe(true);
      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "multi approval complete",
        ),
      ).toBe(true);
      expect(server.capturedRequests()).toHaveLength(2);
      expect(
        events.some(
          (event) =>
            event.type === "agent_end" &&
            Boolean((event as { error?: unknown }).error),
        ),
      ).toBe(false);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
