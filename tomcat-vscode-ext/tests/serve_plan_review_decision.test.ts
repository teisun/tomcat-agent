import { readFile } from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import type { AskQuestionWireRequest, AskQuestionResult } from "../src/serveClient/protocol";
import { createRealServeMessenger, ensureTomcatBinary, responsesCompleted, responsesFunctionCallAdded, responsesFunctionCallArgumentsDelta, responsesTextDelta, spawnScriptedOpenAiStreamServer, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const args = JSON.stringify({ goal: "Review consent integration", draft: "Inspect the current implementation; make the smallest change.", todos: [{ id: "work", content: "Implement", status: "pending" }] });
const createReply = { parts: [responsesFunctionCallAdded("fc_plan", "call_plan", "create_plan"), responsesFunctionCallArgumentsDelta("fc_plan", args), responsesCompleted()] };
const reviewerReply = { parts: [responsesTextDelta("<review>\nsummary: independently checked\nchanges_summary: none\napplied_changes: false\n</review>"), responsesCompleted()] };
const doneReply = { parts: [responsesTextDelta("Saved plan is available."), responsesCompleted()] };
const choose = (request: AskQuestionWireRequest, option: string): AskQuestionResult => ({ cancelled: false, outcome: "answered", answers: [{ questionId: request.questions[0].id, optionIds: [option], pickedRecommended: option === "review" }] });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
function reviewResults(payload: unknown) {
  const records = (payload as { messages: Record<string, unknown>[] }).messages;
  return records.filter(row => row.type === "custom" && row.event === "plan.review");
}

describe("real Serve plan-review consent", () => {
  it.each([{ option: "skip", locale: "zh-CN", requests: 2 }, { option: "review", locale: "en", requests: 3 }])("asks before $option, persists once and does not implicitly authorize execution ($locale)", async ({ option, locale, requests }) => {
    const english = JSON.parse(await readFile(path.resolve(__dirname, "../../tomcat/assets/i18n/en.json"), "utf8")) as Record<string, string>;
    const server = await spawnScriptedOpenAiStreamServer(option === "review" ? [createReply, reviewerReply, doneReply] : [createReply, doneReply]);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");
    let seen = 0;
    const created: { path?: string } = {};
    const eventSubscription = runtime.messenger.onEvent(event => { if (event.type === "plan.create" && event.path) created.path = event.path; });
    try {
      const init = await initializeServe(runtime.messenger);
      await runtime.messenger.request({ type: "set_ui_language", language: locale as "en" | "zh-CN" });
      runtime.messenger.registerAskQuestionHandler(async request => {
        seen++;
        expect(request.toolCallId).toBe("call_plan");
        expect(request.sessionId).toBe(init.sessionId);
        expect(request.questions).toHaveLength(1);
        expect(request.questions[0].allowCustom).toBe(false);
        expect(request.questions[0].options.map(o => o.id)).toEqual(["review", "skip"]);
        if (locale === "en") expect(request.questions[0].prompt).toBe(english["plan.review.prompt"]);
        expect(server.capturedNonTitleRequests()).toHaveLength(1);
        const file = created.path!.replace(/^~(?=\/)/, runtime.fixture.homePath);
        expect(await readFile(file, "utf8")).toContain("state: planning");
        return choose(request, option);
      });
      expect((await runtime.messenger.request({ type: "set_plan_mode", action: "enter", sessionId: init.sessionId })).success).toBe(true);
      const completed = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === init.sessionId, 20_000);
      await runtime.messenger.request({ type: "prompt", sessionId: init.sessionId, params: {}, text: "Create a short plan." });
      await completed;
      const history = await runtime.messenger.request({ type: "get_messages", sessionId: init.sessionId, params: {} });
      const reviews = reviewResults(history.payload);
      expect(seen).toBe(1);
      expect(reviews).toHaveLength(1);
      expect(reviews[0].reviewer_stop_reason).toBe(option === "review" ? "completed" : "user_skipped");
      expect(server.capturedNonTitleRequests()).toHaveLength(requests);
      const state = await runtime.messenger.request({ type: "get_state", sessionId: init.sessionId });
      expect(state.payload).toMatchObject({ agentMode: "plan", activePlan: { state: "planning" } });
    } finally { eventSubscription.dispose(); await runtime.cleanup(); await server.close(); }
  }, 40_000);

  it("Stop during the decision keeps the file, emits parent_abort and discards the late answer", async () => {
    const server = await spawnScriptedOpenAiStreamServer([createReply]);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");
    const arrived = deferred<AskQuestionWireRequest>();
    const answer = deferred<AskQuestionResult>();
    runtime.messenger.registerAskQuestionHandler(request => { arrived.resolve(request); return answer.promise; });
    try {
      const init = await initializeServe(runtime.messenger);
      await runtime.messenger.request({ type: "set_plan_mode", action: "enter", sessionId: init.sessionId });
      await runtime.messenger.request({ type: "prompt", sessionId: init.sessionId, params: {}, text: "Create a plan and wait." });
      const request = await arrived.promise;
      const stopped = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === init.sessionId);
      expect((await runtime.messenger.request({ type: "interrupt", sessionId: init.sessionId })).success).toBe(true);
      await stopped;
      answer.resolve(choose(request, "review"));
      const history = await runtime.messenger.request({ type: "get_messages", sessionId: init.sessionId, params: {} });
      expect(reviewResults(history.payload)).toMatchObject([{ reviewer_stop_reason: "parent_abort" }]);
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 30_000);

  it("host disconnect ends the old decision; reconnect does not replay create_plan", async () => {
    const server = await spawnScriptedOpenAiStreamServer([createReply]);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");
    const arrived = deferred<AskQuestionWireRequest>();
    const answer = deferred<AskQuestionResult>();
    runtime.messenger.registerAskQuestionHandler(request => { arrived.resolve(request); return answer.promise; });
    let replacement: TomcatMessenger | undefined;
    try {
      const init = await initializeServe(runtime.messenger);
      await runtime.messenger.request({ type: "set_plan_mode", action: "enter", sessionId: init.sessionId });
      await runtime.messenger.request({ type: "prompt", sessionId: init.sessionId, params: {}, text: "Create a plan and wait." });
      const request = await arrived.promise;
      await runtime.messenger.disposeAsync();
      answer.resolve(choose(request, "review"));
      replacement = new TomcatMessenger({ executable: await ensureTomcatBinary(), env: runtime.fixture.env, cwd: runtime.fixture.workspacePath });
      let reasked = 0;
      replacement.registerAskQuestionHandler(() => { reasked++; return { answers: [], cancelled: true, outcome: "skipped" }; });
      const reopened = await initializeServe(replacement);
      const history = await replacement.request({ type: "get_messages", sessionId: reopened.sessionId, params: {} });
      expect(reviewResults(history.payload)).toMatchObject([{ reviewer_stop_reason: "parent_abort" }]);
      const state = await replacement.request({ type: "get_state", sessionId: reopened.sessionId });
      expect(state.payload).toMatchObject({ activePlan: { state: "planning" } });
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
      expect(reasked).toBe(0);
    } finally { await replacement?.disposeAsync(); await runtime.cleanup(); await server.close(); }
  }, 30_000);
});
