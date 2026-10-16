import { describe, expect, it } from "vitest";
import { readFile } from "node:fs/promises";
import * as path from "node:path";

import { initializeServe } from "../src/serveClient/initialize";
import type { AskQuestionResult } from "../src/serveClient/protocol";
import {
  createRealServeMessenger,
  responsesCompleted,
  responsesFunctionCallAdded,
  responsesFunctionCallArgumentsDelta,
  responsesTextDelta,
  spawnScriptedOpenAiStreamServer,
  waitForEvent,
  warmTomcatBinaryForSuite,
  writePlanFile,
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
  it.each([false, true])("starts a plan and routes execution questions through Host (stop=%s)", async (stop) => {
    const replies: Parameters<typeof spawnScriptedOpenAiStreamServer>[0] = [];
    const server = await spawnScriptedOpenAiStreamServer(replies);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");
    let releaseAnswer: ((value: AskQuestionResult) => void) | undefined;
    try {
      const plan = await writePlanFile(runtime.fixture.homePath, "host-start-plan");
      const written = path.join(runtime.fixture.homePath, ".tomcat/temp/host-plan-result.txt");
      const tool = (id: string, name: string, args: unknown) => ({ parts: [responsesFunctionCallAdded(`fc-${id}`, id, name), responsesFunctionCallArgumentsDelta(`fc-${id}`, JSON.stringify(args)), responsesCompleted()] });
      replies.push(
        tool("start", "start_plan", { plan_id: "host-start-plan" }),
        tool("write", "write", { path: written, content: "written during execution" }),
        tool("ask", "ask_question", JSON.parse(ASK_QUESTION_ARGS)),
        tool("complete", "update_plan", { ops: [{ kind: "set_status", id: "step1", status: "completed", evidence: ["Host answered"] }] }),
        { parts: [responsesTextDelta("host execution finished"), responsesCompleted()] },
      );
      let sawQuestion!: () => void;
      const questionReady = new Promise<void>(resolve => { sawQuestion = resolve; });
      let confirmation: unknown;
      runtime.messenger.registerAskQuestionHandler(async request => {
        if (request.questions[0].id === "plan-start") {
          confirmation = { allowCustom: request.questions[0].allowCustom, toolCallId: request.toolCallId };
          return { answers: [{ questionId: "plan-start", optionIds: ["start"], pickedRecommended: true }], cancelled: false, outcome: "answered" };
        }
        expect(request.toolCallId).toBe("ask");
        return new Promise<AskQuestionResult>(resolve => { releaseAnswer = resolve; sawQuestion(); });
      });
      const init = await initializeServe(runtime.messenger);
      await runtime.messenger.sendSetPlanMode({ action: "enter", sessionId: init.sessionId });
      const ended = waitForEvent(runtime.messenger, event => event.type === "agent_idle", 15_000);
      void ended.catch(() => undefined);
      await runtime.messenger.request({ type: "prompt", sessionId: init.sessionId, text: "Implement the saved plan now", params: {} });
      await Promise.race([questionReady, ended.then(() => { throw new Error("Turn ended before its execution question"); })]);
      const toolOutputs = server.capturedNonTitleRequests().flatMap(raw => JSON.parse(raw.split("\r\n\r\n")[1]).input.filter((item: { type?: string }) => item.type === "function_call_output"));
      expect(confirmation, JSON.stringify(toolOutputs)).toEqual({ allowCustom: false, toolCallId: "start" });
      expect(await readFile(plan, "utf8"), JSON.stringify(toolOutputs)).toContain("state: executing");
      expect(await readFile(written, "utf8").catch(error => String(error)), JSON.stringify(toolOutputs)).toBe("written during execution");
      if (stop) await runtime.messenger.request({ type: "interrupt", sessionId: init.sessionId });
      releaseAnswer!({ answers: [{ questionId: "q1", optionIds: ["a"], pickedRecommended: true }], cancelled: false, outcome: "answered" });
      const events = await ended;
      expect(await readFile(plan, "utf8")).toContain(`state: ${stop ? "pending" : "completed"}`);
      expect(server.capturedNonTitleRequests()).toHaveLength(stop ? 3 : 5);
      if (stop) expect(events.some(event => event.type === "agent_interrupted")).toBe(true);
      else expect(events.some(event => event.type === "message_update" && (event.assistantMessageEvent as { delta?: string }).delta === "host execution finished")).toBe(true);
    } finally {
      releaseAnswer?.({ answers: [], cancelled: true });
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);

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
      expect(server.capturedNonTitleRequests()).toHaveLength(2);
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
      expect(server.capturedNonTitleRequests()).toHaveLength(2);
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
