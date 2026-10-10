import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../provider";
import type { WebviewStateSnapshot } from "../protocol";

const commands = ["reload", "install", "uninstall"].map((name) => ({name, usage:`/${name}`, summary:name}));
function fixture(runSlashCommand = vi.fn().mockResolvedValue({ok:true, text:"同步完成"}), compact = vi.fn().mockResolvedValue({beforeUsageRatio:0.8, afterUsageRatio:0.2})) {
  const provider = new TomcatWebviewViewProvider({
    extensionUri:vscode.Uri.file("/extension"), getDefaultCwd:()=>"/workspace", ide:{} as never,
    initialize:async()=>({attachmentRoot:null, capabilities:["run_slash_command"], protocolVersion:2, sessionId:"s1", serverVersion:"test", slashCommands:commands}),
    messenger:{onEvent:()=>({dispose(){}})} as never, sessionRouter:{runSlashCommand,compact} as never,
  });
  // Isolate host command orchestration; real Serve and SessionPool are covered by integration.
  const host = provider as any;
  const frames:WebviewStateSnapshot[] = [];
  host.stateStore.setActiveSession("s1");
  host.stateStore.setReady(true);
  host.ensureWebviewSessionWithoutHistory = vi.fn(async()=>"s1");
  host.refreshSessionState = vi.fn(async()=>undefined);
  host.refreshSessionHistory = vi.fn(async()=>undefined);
  host.postState = vi.fn(async()=>{frames.push(provider.currentState());});
  const send = (type = "runSlashCommand", text = "/reload") => host.handleIntent({messageId:"cmd", type, data:{sessionId:"s1",text}});
  return {provider,host,frames,send,runSlashCommand,compact};
}

describe("host shared command replies", () => {
  it.each([true,false])("writes a UI-only %s reply and broadcasts the backend table", async (ok) => {
    const f = fixture(vi.fn().mockResolvedValue({ok, text:"reply"}));
    try {
      await f.send();
      const state = f.provider.currentState();
      expect(state.slashCommands).toEqual(commands);
      expect(state.sessionViews.s1.timeline).toEqual([expect.objectContaining({type:"message",kind:ok?"notice":"error",text:"reply"})]);
      expect(f.frames.some((frame)=>frame.sessionViews.s1.commandPending === true)).toBe(true);
      expect(state.sessionViews.s1.commandPending).toBe(false);
      expect(state.sessionViews.s1.busy).toBe(false);
      expect(f.runSlashCommand).toHaveBeenCalledWith("s1", "/reload");
    } finally { f.provider.dispose(); }
  });
  it("keeps commandPending until reply, rejects duplicates and does not clear it on ordinary idle", async () => {
    let resolve!:(value:{ok:boolean;text:string})=>void;
    const reply = new Promise<{ok:boolean;text:string}>((done)=>{resolve=done;});
    const f = fixture(vi.fn().mockReturnValue(reply));
    try {
      const pending = f.send();
      await vi.waitFor(()=>expect(f.runSlashCommand).toHaveBeenCalledTimes(1));
      f.host.stateStore.applyEvent({type:"agent_idle",sessionId:"s1"});
      expect(f.provider.currentState().sessionViews.s1.commandPending).toBe(true);
      await f.send(); expect(f.runSlashCommand).toHaveBeenCalledTimes(1);
      resolve({ok:true,text:"done"}); await pending;
      expect(f.provider.currentState().sessionViews.s1.commandPending).toBe(false);
    } finally { resolve({ok:true,text:"cleanup"}); f.provider.dispose(); }
  });
  it.each(["busy","transport disconnected"])("clears a rejected request and shows an error: %s", async (detail) => {
    const f = fixture(vi.fn().mockRejectedValue(new Error(detail)));
    try {
      await f.send(); const state = f.provider.currentState();
      expect(state.sessionViews.s1.commandPending).toBe(false);
      expect(state.sessionViews.s1.timeline.at(-1)).toMatchObject({kind:"error"});
      if(detail === "busy") expect(state.sessionViews.s1.timeline.at(-1)).toMatchObject({text:expect.stringContaining("Wait until it finishes")});
    } finally { f.provider.dispose(); }
  });
  it("shares pending/finally behavior with compact and refreshes its transcript", async () => {
    const f = fixture();
    try {
      await f.send("compact");
      expect(f.frames.some((frame)=>frame.sessionViews.s1.commandPending)).toBe(true);
      expect(f.provider.currentState().sessionViews.s1.commandPending).toBe(false);
      expect(f.host.refreshSessionHistory).toHaveBeenCalledWith("s1");
      expect(f.provider.currentState().sessionViews.s1.timeline.at(-1)).toMatchObject({kind:"notice",text:expect.stringContaining("Context compacted")});
    } finally { f.provider.dispose(); }
  });
});
