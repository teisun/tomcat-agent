import * as vscode from "vscode";
import { describe, expect, it, vi } from "vitest";
import { TomcatWebviewViewProvider } from "../provider";

function fixture(capabilities: string[], request = vi.fn().mockResolvedValue({success:true,sessionId:"s1",payload:{items:[]}})) {
  const provider = new TomcatWebviewViewProvider({extensionUri:vscode.Uri.file("/extension"),getDefaultCwd:()=>"/workspace",ide:{} as never,initialize:async()=>({attachmentRoot:null,capabilities,protocolVersion:2,sessionId:"s1",serverVersion:"test"}),messenger:{request,onEvent:()=>({dispose(){}})} as never,sessionRouter:{} as never});
  const host = provider as any;
  host.stateStore.setActiveSession("s1");host.stateStore.setReady(true);
  host.postState=vi.fn(async()=>undefined);host.refreshSessions=vi.fn(async()=>undefined);host.refreshSessionState=vi.fn(async()=>undefined);
  return {provider,host,request};
}

describe("instruction transport",()=>{
  it.each([true,false])("strips draft identity and preserves failed drafts (success=%s)",async success=>{
    const f=fixture(["get_instruction_catalog"],vi.fn().mockResolvedValue({success,error:success?undefined:"file removed"}));
    const instruction={type:"instruction",kind:"command",label:"/review",resourceId:"command:.cursor/commands/review.md",occurrenceId:"draft-only",path:"display-only"};
    try {
      await f.host.ensureInitialized();
      await f.host.sendUserMessage("s1","prompt","/review",[instruction]);
      expect(f.request).toHaveBeenCalledWith(expect.objectContaining({params:expect.objectContaining({segments:[{type:"instruction",kind:"command",label:"/review",resourceId:"command:.cursor/commands/review.md"}]})}));
      expect(JSON.stringify(f.request.mock.calls)).not.toContain("draft-only");
      expect(f.host.draftStore.peek("s1").segments.length).toBe(success?0:1);
    } finally {f.provider.dispose();}
  });
  it("rejects restored invocation chips on an old server without losing their draft",async()=>{
    const f=fixture([]);
    try {
      await f.host.ensureInitialized();
      await f.host.sendUserMessage("s1","prompt","/review",[{type:"instruction",kind:"command",label:"/review",resourceId:"command:x",occurrenceId:"retained"}]);
      expect(f.request).not.toHaveBeenCalled();
      expect(f.host.draftStore.peek("s1").segments[0].occurrenceId).toBe("retained");
      expect(f.provider.currentState().sessionViews.s1.timeline.at(-1)).toMatchObject({deliveryState:"failed"});
    } finally {f.provider.dispose();}
  });
  it("keeps a successful action usable if catalog refresh fails",async()=>{
    const f=fixture(["get_instruction_catalog"],vi.fn().mockRejectedValue(new Error("offline")));
    const warn=vi.spyOn(console,"warn").mockImplementation(()=>undefined);
    try {await expect(f.host.refreshInstructionCatalog("s1")).resolves.toBeUndefined();expect(f.provider.currentState().activeSessionId).toBe("s1");}
    finally {warn.mockRestore();f.provider.dispose();}
  });
});
