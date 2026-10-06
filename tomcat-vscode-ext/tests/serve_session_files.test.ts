import { readFile, readdir, writeFile, access } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { createRealServeMessenger, spawnScriptedOpenAiStreamServer, sseDelta, sseDone, sseFinish, sseToolCall, waitForEvent, warmTomcatBinaryForSuite, type ScriptedResponse } from "./serveTestUtils";

warmTomcatBinaryForSuite(300_000);
describe("real Serve session files",()=>{
  it("keeps files through questions, switches the editing turn and restores only that turn without chatting",async()=>{
    const scripts:ScriptedResponse[]=[];
    const server=await spawnScriptedOpenAiStreamServer(scripts);
    const runtime=await createRealServeMessenger(server.baseUrl);
    try {
      const a=path.join(runtime.fixture.workspacePath,"a.txt"), b=path.join(runtime.fixture.workspacePath,"b.txt"), c=path.join(runtime.fixture.workspacePath,"c.txt");
      await writeFile(b,"B0\n");
      const tool=(id:string,name:string,args:unknown):ScriptedResponse=>({parts:[sseToolCall(id,name,JSON.stringify(args)),sseFinish("tool_calls"),sseDone()]});
      const done=():ScriptedResponse=>({parts:[sseDelta("Done"),sseFinish("stop"),sseDone()]});
      scripts.push(tool("read1","read",{path:b}),tool("edit1","write",{path:b,content:"B1\n",overwrite:true}),tool("new1","write",{path:a,content:"A1\n"}),done(),done(),done(),tool("read4","read",{path:b}),tool("edit4","write",{path:b,content:"B2\n",overwrite:true}),tool("new4","write",{path:c,content:"C\n"}),done(),done());
      runtime.messenger.registerControlRequestHandler("confirmation",frame=>({kind:"response",payload:{decision:"allow_once"},sessionId:frame.sessionId}));
      const init=await initializeServe(runtime.messenger);expect(init.capabilities).toContain("session_files");
      const sessionId=init.sessionId!;const router=new SessionRouter(runtime.messenger,()=>runtime.fixture.workspacePath);
      async function turn(id:string,text:string) {
        const idle=waitForEvent(runtime.messenger,e=>e.type==="agent_idle" && e.sessionId===sessionId,30_000);idle.catch(()=>undefined);
        const response=await runtime.messenger.request({type:"prompt",sessionId,text,params:{userMessageId:id}});
        expect(response.success).toBe(true);await idle;
      }
      await turn("u1","Make the first edits.");
      const first=await router.getSessionFiles(sessionId);
      expect(first.sourceTurnId).toBe("u1");expect(first.files.map(f=>path.basename(f.path)).sort()).toEqual(["a.txt","b.txt"]);
      await turn("u2","Why did you do that?");await turn("u3","Explain more, no edits.");
      expect((await router.getSessionFiles(sessionId)).sourceTurnId).toBe("u1");
      await turn("u4","Make a second edit and create c.");
      const second=await router.getSessionFiles(sessionId);
      expect(second.sourceTurnId).toBe("u4");expect(second.files.map(f=>path.basename(f.path)).sort()).toEqual(["b.txt","c.txt"]);
      const bPath=second.files.find(f=>path.basename(f.path)==="b.txt")!.path;
      const cPath=second.files.find(f=>path.basename(f.path)==="c.txt")!.path;
      expect((await router.getSessionFileBaseline(sessionId,"u4",bPath)).text).toBe("B1\n");
      const entries=await readdir(path.join(runtime.fixture.homePath,".tomcat"),{recursive:true});
      const transcript=entries.find(name=>name.endsWith(`${sessionId}.jsonl`));expect(transcript).toBeTruthy();
      const transcriptPath=path.join(runtime.fixture.homePath,".tomcat",transcript!);
      const before=await readFile(transcriptPath);const requests=server.capturedNonTitleRequests().length;
      await router.restoreSessionFiles(sessionId,"u4",[bPath]);
      expect(await readFile(b,"utf8")).toBe("B1\n");expect(await readFile(a,"utf8")).toBe("A1\n");
      await router.restoreSessionFiles(sessionId,"u4",[cPath]);await expect(access(c)).rejects.toThrow();
      expect(await readFile(transcriptPath)).toEqual(before);expect(server.capturedNonTitleRequests()).toHaveLength(requests);
      expect(await router.getSessionFiles(sessionId)).toMatchObject({sourceTurnId:"u4",files:[]});
      await turn("u5","Thanks, another question only.");
      expect(await router.getSessionFiles(sessionId)).toMatchObject({sourceTurnId:"u4",files:[]});
    }finally{await runtime.cleanup();await server.close();}
  },90_000);
});
