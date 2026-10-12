import { mkdir, unlink, writeFile } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { createRealServeMessenger, spawnScriptedOpenAiStreamServer, sseDelta, sseDone, sseFinish, responsesTextDelta, responsesCompleted, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
describe("real Serve instruction invocation",()=>{
  it("locks a busy follow_up command before the real queue is drained", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("FIRST_TURN_BUSY"), { ...sseFinish("stop"), delayMs: 5000 }, sseDone()] },
      { parts: [sseDelta("FOLLOW_UP_DONE"), sseFinish("stop"), sseDone()] },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    try {
      const directory = path.join(runtime.fixture.workspacePath, ".cursor/commands");
      await mkdir(directory, { recursive: true });
      const command = path.join(directory, "review.md");
      await writeFile(command, "FOLLOW_UP_BODY_OLD");
      const init = await initializeServe(runtime.messenger);
      const sessionId = init.sessionId!;
      const started = waitForEvent(runtime.messenger, e => e.type === "message_update" && e.sessionId === sessionId);
      const idle = waitForEvent(runtime.messenger, e => e.type === "agent_idle" && e.sessionId === sessionId, 15000);
      // Own both waits even if a request fails before we await the corresponding event.
      started.catch(() => undefined); idle.catch(() => undefined);
      const first = await runtime.messenger.request({ type: "prompt", sessionId, text: "Hold the first turn", params: {} });
      expect(first.success).toBe(true);
      await started;
      const queued = await runtime.messenger.request({
        type: "follow_up", sessionId, text: "/review FOLLOW_UP_UNIQUE_INTENT",
        params: { userMessageId: "queued-command-fixed", segments: [
          { type: "instruction", kind: "command", resourceId: "command:.cursor/commands/review.md", label: "/review" },
          { type: "text", text: " FOLLOW_UP_UNIQUE_INTENT" },
        ] },
      });
      expect(queued.success).toBe(true);
      expect(queued.payload).toMatchObject({ queued: true });
      await writeFile(command, "FOLLOW_UP_BODY_NEW");
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
      await idle;
      const requests = server.capturedNonTitleRequests();
      expect(requests).toHaveLength(2);
      const payload = JSON.parse(requests[1].split("\r\n\r\n")[1]);
      const followUp = payload.messages.find((message: { role: string; content: unknown }) =>
        message.role === "user" && JSON.stringify(message.content).includes("FOLLOW_UP_UNIQUE_INTENT"));
      expect(followUp).toBeDefined();
      expect(JSON.stringify(followUp.content)).toContain("FOLLOW_UP_BODY_OLD");
      expect(JSON.stringify(followUp.content)).not.toContain("FOLLOW_UP_BODY_NEW");
    } finally { await runtime.cleanup(); await server.close(); }
  }, 45000);
  it.each(["openai","openai-responses"] as const)("sends rules and frozen command snapshots to %s, preserves them on retry",async api=>{
    const parts = api === "openai" ? [sseDelta("done"),sseFinish("stop"),sseDone()] : [responsesTextDelta("done"),responsesCompleted()];
    const server=await spawnScriptedOpenAiStreamServer([{parts},{parts}]);
    const runtime=await createRealServeMessenger(server.baseUrl,api);
    try {
      const commands=path.join(runtime.fixture.workspacePath,".cursor/commands");
      const rules=path.join(runtime.fixture.workspacePath,".agents/rules");
      await mkdir(commands,{recursive:true});await mkdir(rules,{recursive:true});
      const command=path.join(commands,"review.md");
      await writeFile(command,"---\ndescription: Review\n---\nCOMMAND_BODY_ORIGINAL");
      await writeFile(path.join(rules,"team.md"),"---\nalwaysApply: true\n---\nTEAM_RULE_ORIGINAL");
      const init=await initializeServe(runtime.messenger); const sessionId=init.sessionId!;
      expect(init.capabilities).toContain("get_instruction_catalog");
      const catalog=await runtime.messenger.request({type:"get_instruction_catalog",sessionId});
      expect(catalog.success).toBe(true);
      expect(JSON.stringify(catalog.payload)).toContain("command:.cursor/commands/review.md");
      expect(JSON.stringify(catalog.payload)).not.toContain("COMMAND_BODY_ORIGINAL");
      const idle=waitForEvent(runtime.messenger,e=>e.type === "agent_idle" && e.sessionId === sessionId);
      const sent=await runtime.messenger.request({type:"prompt",sessionId,text:"/review",params:{userMessageId:"command-message",segments:[{type:"instruction",kind:"command",resourceId:"command:.cursor/commands/review.md",label:"/review"},{type:"text",text:" Check tests"}]}});
      expect(sent.success).toBe(true);await idle;
      const raw=server.capturedNonTitleRequests()[0];
      expect(raw).toContain("COMMAND_BODY_ORIGINAL");expect(raw).toContain("TEAM_RULE_ORIGINAL");
      expect(raw).toContain("<command name=");
      const history=await runtime.messenger.request({type:"get_messages",sessionId});
      expect(JSON.stringify(history.payload)).toContain('"ref_kind":"command"');
      expect(JSON.stringify(history.payload)).toContain("COMMAND_BODY_ORIGINAL");
      await unlink(command);
      await writeFile(path.join(rules,"team.md"),"---\nalwaysApply: true\n---\nTEAM_RULE_CHANGED");
      const secondIdle=waitForEvent(runtime.messenger,e=>e.type === "agent_idle" && e.sessionId === sessionId);
      const retry=await runtime.messenger.request({type:"retry",sessionId,messageId:"command-message"});
      expect(retry.success).toBe(true);await secondIdle;
      const second=server.capturedNonTitleRequests().at(-1)!;
      expect(second).toContain("COMMAND_BODY_ORIGINAL");expect(second).toContain("TEAM_RULE_CHANGED");
      const before=await runtime.messenger.request({type:"get_messages",sessionId});
      const rejected=await runtime.messenger.request({type:"prompt",sessionId,text:"",params:{segments:[{type:"instruction",kind:"command",resourceId:"command:.cursor/commands/review.md",label:"/review"}]}});
      expect(rejected.success).toBe(false);
      const after=await runtime.messenger.request({type:"get_messages",sessionId});
      expect(after.payload).toEqual(before.payload);
    } finally {await runtime.cleanup();await server.close();}
  },45000);
});
