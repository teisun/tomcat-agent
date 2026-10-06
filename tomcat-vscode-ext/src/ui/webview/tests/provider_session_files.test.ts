import * as fs from "node:fs";
import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { TomcatWebviewViewProvider } from "../provider";
import { isWebviewIntent } from "../protocol";

const __testing = (vscode as typeof vscode & {
  __testing: { fireDidSaveTextDocument(document: vscode.TextDocument): void };
}).__testing;

function fixture(supported = true) {
  const getSessionFiles = vi.fn().mockResolvedValue({ sessionId:"s1", sourceTurnId:"u1", files:[{path:"/workspace/a.ts", status:"modified", added:1, removed:1, restorable:true}] });
  const getSessionFileBaseline=vi.fn().mockResolvedValue({sessionId:"s1",sourceTurnId:"u1",path:"/workspace/a.ts",existed:true,text:"before"});
  const restoreSessionFiles=vi.fn().mockResolvedValue({sessionId:"s1",sourceTurnId:"u1",restored:["/workspace/a.ts"]});
  const openSessionFileDiff=vi.fn();
  const initialized={capabilities:supported?["session_files"]:[],protocolVersion:2,sessionId:"s1",serverVersion:"test",attachmentRoot:null};
  const provider=new TomcatWebviewViewProvider({extensionUri:vscode.Uri.file("/extension"),getDefaultCwd:()=>"/workspace",ide:{openSessionFileDiff} as never,initialize:async()=>initialized,messenger:{onEvent:()=>({dispose(){}})} as never,sessionRouter:{getSessionFiles,getSessionFileBaseline,restoreSessionFiles} as never});
  const host=provider as any;
  host.initialized=initialized;host.stateStore.setActiveSession("s1");host.postSessionView=vi.fn();host.postState=vi.fn();host.postEvent=vi.fn();
  return {provider,host,getSessionFiles,getSessionFileBaseline,restoreSessionFiles,openSessionFileDiff};
}

describe("session files host state",()=>{
  it("keeps question results, accepts an empty same-turn list, and replaces the whole turn",async()=>{
    const f=fixture();try {
      await f.host.refreshSessionFiles("s1");
      await f.host.refreshSessionFiles("s1"); // question: server source remains u1
      expect(f.provider.currentState().sessionViews.s1.sessionFiles?.sourceTurnId).toBe("u1");
      f.getSessionFiles.mockResolvedValue({sessionId:"s1",sourceTurnId:"u4",files:[]});
      await f.host.refreshSessionFiles("s1");
      expect(f.provider.currentState().sessionViews.s1.sessionFiles).toMatchObject({sourceTurnId:"u4",files:[]});
    } finally {f.provider.dispose();}
  });
  it("coalesces in-flight triggers and discards closed-session replies",async()=>{
    const f=fixture();let resolve!:(v:unknown)=>void;
    f.getSessionFiles.mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
    try {
      const first=f.host.refreshSessionFiles("s1");
      f.host.refreshSessionFiles("s1");f.host.refreshSessionFiles("s1");
      resolve({sessionId:"s1",sourceTurnId:"u1",files:[]});await first;
      expect(f.getSessionFiles).toHaveBeenCalledTimes(2);
      f.getSessionFiles.mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
      const late=f.host.refreshSessionFiles("s1");
      f.host.closedFileSessions.add("s1");
      resolve({sessionId:"s1",sourceTurnId:"u9",files:[]});await late;
      expect(f.provider.currentState().sessionViews.s1.sessionFiles?.sourceTurnId).toBe("u1");
    } finally {f.provider.dispose();}
  });
  it("retains data on failure, projects capability and does not query an old CLI",async()=>{
    const f=fixture();try{
      await f.host.refreshSessionFiles("s1");f.getSessionFiles.mockRejectedValue(new Error("unavailable"));
      await f.host.refreshSessionFiles("s1");
      expect(f.provider.currentState().sessionViews.s1.sessionFiles).toMatchObject({sourceTurnId:"u1",error:"unavailable",files:[expect.objectContaining({path:"/workspace/a.ts"})]});
      expect(f.host.projectCurrentDrafts(f.provider.currentState()).sessionFilesSupported).toBe(true);
    }finally{f.provider.dispose();}
    const old=fixture(false);try {await old.host.refreshSessionFiles("s1");expect(old.getSessionFiles).not.toHaveBeenCalled();expect(old.host.projectCurrentDrafts(old.provider.currentState()).sessionFilesSupported).toBe(false);}finally{old.provider.dispose();}
  });
  it("opens the selected turn's baseline and returns file-only restore results", async () => {
    const f=fixture();try {
      await f.host.handleIntent({messageId:"diff",type:"openSessionFileDiff",data:{sessionId:"s1",sourceTurnId:"u1",path:"/workspace/a.ts"}});
      expect(f.getSessionFileBaseline).toHaveBeenCalledWith("s1","u1","/workspace/a.ts");
      expect(f.openSessionFileDiff).toHaveBeenCalledWith("s1","u1","/workspace/a.ts","before");
      await f.host.handleIntent({messageId:"restore",type:"restoreSessionFiles",data:{sessionId:"s1",sourceTurnId:"u1",paths:["/workspace/a.ts"],requestId:"r"}});
      expect(f.restoreSessionFiles).toHaveBeenCalledWith("s1","u1",["/workspace/a.ts"]);
      expect(f.host.postEvent).toHaveBeenCalledWith(expect.objectContaining({type:"restoreSessionFilesResult",requestId:"r",success:true}));
      expect(f.provider.currentState().sessionViews.s1.commandPending).toBe(false);
      expect(f.provider.currentState().sessionViews.s1.timeline).toEqual([]);
    }finally{f.provider.dispose();}
  });
  it("rejects busy without adding chat messages and preserves the error result", async () => {
    const f=fixture();try {
      f.host.stateStore.applySessionState({sessionId:"s1",busy:true});
      await f.host.handleIntent({messageId:"r",type:"restoreSessionFiles",data:{sessionId:"s1",sourceTurnId:"u1",paths:["/workspace/a.ts"],requestId:"r"}});
      expect(f.restoreSessionFiles).not.toHaveBeenCalled();
      expect(f.host.postEvent).toHaveBeenCalledWith(expect.objectContaining({success:false,error:expect.stringContaining("wait")}));
    }finally{f.provider.dispose();}
  });
  it("blocks only dirty target documents, not unrelated dirty files", async () => {
    const f=fixture();
    const document=await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/a.ts"));
    const dirty=vi.spyOn(document,"isDirty","get").mockReturnValue(true);
    const warning=vi.spyOn(vscode.window,"showWarningMessage").mockImplementation(() => new Promise(() => {}));
    try {
      await f.host.handleIntent({messageId:"r",type:"restoreSessionFiles",data:{sessionId:"s1",sourceTurnId:"u1",paths:["/workspace/a.ts"],requestId:"dirty"}});
      expect(f.restoreSessionFiles).not.toHaveBeenCalled();
      expect(warning).toHaveBeenCalledWith(expect.stringContaining("unsaved changes in a.ts"));
      await f.host.handleIntent({messageId:"r2",type:"restoreSessionFiles",data:{sessionId:"s1",sourceTurnId:"u1",paths:["/workspace/other.ts"],requestId:"other"}});
      expect(f.restoreSessionFiles).toHaveBeenCalledWith("s1","u1",["/workspace/other.ts"]);
    } finally {dirty.mockRestore();f.provider.dispose();}
  });
  it("refreshes once for a saved target, not unrelated or non-file documents", async () => {
    const f=fixture();try {
      await f.host.refreshSessionFiles("s1");f.getSessionFiles.mockClear();
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/other.ts")));
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.parse("untitled:/workspace/a.ts")));
      expect(f.getSessionFiles).not.toHaveBeenCalled();
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/a.ts")));
      await vi.waitFor(()=>expect(f.getSessionFiles).toHaveBeenCalledTimes(1));
      await f.host.fileRefreshes.get("s1")?.promise;
      f.host.stateStore.setSessionFiles("s1",{sourceTurnId:"u1",files:[]});
      f.getSessionFiles.mockClear();
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/a.ts")));
      expect(f.getSessionFiles).not.toHaveBeenCalled();
    }finally{f.provider.dispose();}
  });
  it("matches realpath aliases and uses the same fallback identity as dirty protection", async () => {
    const f=fixture();const realpath=vi.spyOn(fs.realpathSync,"native").mockImplementation(value => {
      const text=String(value);
      if(text==="/workspace/alias.ts" || text==="/workspace/a.ts") return "/private/workspace/a.ts";
      throw new Error("missing");
    });
    try {
      await f.host.refreshSessionFiles("s1");f.getSessionFiles.mockClear();
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/alias.ts")));
      await vi.waitFor(()=>expect(f.getSessionFiles).toHaveBeenCalledTimes(1));
      await f.host.fileRefreshes.get("s1")?.promise;
      realpath.mockImplementation(()=>{throw new Error("missing");});f.getSessionFiles.mockClear();
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/a.ts")));
      await vi.waitFor(()=>expect(f.getSessionFiles).toHaveBeenCalledTimes(1));
    }finally{realpath.mockRestore();f.provider.dispose();}
  });
  it("checks only the active session's cached paths when saving", async () => {
    const f=fixture();try {
      await f.host.refreshSessionFiles("s1");f.getSessionFiles.mockClear();
      f.host.stateStore.setActiveSession("s2");
      f.host.stateStore.setSessionFiles("s2",{sourceTurnId:"u2",files:[{path:"/workspace/b.ts",status:"modified",restorable:true}]});
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/a.ts")));
      expect(f.getSessionFiles).not.toHaveBeenCalled();
      f.getSessionFiles.mockResolvedValue({sessionId:"s2",sourceTurnId:"u2",files:[]});
      __testing.fireDidSaveTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file("/workspace/b.ts")));
      await vi.waitFor(()=>expect(f.getSessionFiles).toHaveBeenCalledExactlyOnceWith("s2"));
    }finally{f.provider.dispose();}
  });
  it.each([
    ["binary", "a.ts is a binary file, so there is no text diff."],
    ["too_large", "a.ts is too large to compare."],
    ["unavailable", "The original copy of a.ts is missing."],
    ["unknown_path", "This editing turn is no longer available. Files has been refreshed."],
    ["head_moved", "Git HEAD changed since this backup. View the diff only."],
    ["not_regular_file", "a.ts is not a regular file and cannot be undone."],
    ["Timed out waiting for response", "Unable to open diff /workspace/a.ts: Tomcat bridge is not responding. Restart Tomcat and try again."],
    ["tomcat serve exited", "Unable to open diff /workspace/a.ts: Tomcat serve exited. Restart Tomcat and try again."],
    ["unexpected failure", "Unable to open diff /workspace/a.ts: unexpected failure"],
  ])("shows readable diff failure %s and refreshes without waiting for the warning",async(code,expected)=>{
    const f=fixture();const warning=vi.spyOn(vscode.window,"showWarningMessage").mockImplementation(()=>new Promise(()=>{}));
    f.getSessionFileBaseline.mockRejectedValue(new Error(code));
    try {
      await f.host.handleIntent({messageId:"diff-error",type:"openSessionFileDiff",data:{sessionId:"s1",sourceTurnId:"u1",path:"/workspace/a.ts"}});
      expect(warning).toHaveBeenCalledWith(expected);
      expect(f.openSessionFileDiff).not.toHaveBeenCalled();
      await vi.waitFor(()=>expect(f.getSessionFiles).toHaveBeenCalledTimes(1));
      expect(f.provider.currentState().sessionViews.s1.timeline).toEqual([]);
    }finally{warning.mockRestore();f.provider.dispose();}
  });
  it.each([
    ["head_moved", "Git HEAD changed since this backup. View the diff only."],
    ["unavailable", "The original copy of a.ts is missing."],
    ["unknown_path", "This editing turn is no longer available. Files has been refreshed."],
    ["not_regular_file", "a.ts is not a regular file and cannot be undone."],
    ["busy", "Stop Tomcat or wait for it to finish to undo."],
    ["restore_failed: /workspace/a.ts: Permission denied: read-only", "Couldn't undo /workspace/a.ts: Permission denied: read-only. The list shows what's left."],
    ["tomcat serve exited", "Unable to undo file changes: Tomcat serve exited. Restart Tomcat and try again."],
  ])("returns readable restore failure %s, clears pending, and refreshes",async(code,expected)=>{
    const f=fixture();const warning=vi.spyOn(vscode.window,"showWarningMessage").mockImplementation(()=>new Promise(()=>{}));
    f.restoreSessionFiles.mockRejectedValue(new Error(code));
    try {
      await f.host.handleIntent({messageId:"undo-error",type:"restoreSessionFiles",data:{sessionId:"s1",sourceTurnId:"u1",paths:["/workspace/a.ts"],requestId:"undo-error"}});
      expect(warning).toHaveBeenCalledWith(expected);
      expect(f.host.postEvent).toHaveBeenCalledWith(expect.objectContaining({type:"restoreSessionFilesResult",requestId:"undo-error",success:false,error:expected}));
      expect(f.getSessionFiles).toHaveBeenCalledTimes(1);
      expect(f.provider.currentState().sessionViews.s1.commandPending).toBe(false);
      expect(f.provider.currentState().sessionViews.s1.timeline).toEqual([]);
    }finally{warning.mockRestore();f.provider.dispose();}
  });
  it("maps IDE errors too and preserves the unknown non-Error fallback",async()=>{
    const f=fixture();const warning=vi.spyOn(vscode.window,"showWarningMessage").mockResolvedValue(undefined);
    f.openSessionFileDiff.mockRejectedValueOnce(new Error("binary"));
    try {
      const intent={messageId:"ide-error",type:"openSessionFileDiff",data:{sessionId:"s1",sourceTurnId:"u1",path:"/workspace/a.ts"}};
      await f.host.handleIntent(intent);
      expect(warning).toHaveBeenLastCalledWith("a.ts is a binary file, so there is no text diff.");
      await f.host.fileRefreshes.get("s1")?.promise;f.getSessionFiles.mockClear();
      f.getSessionFileBaseline.mockRejectedValueOnce("unexpected non-Error");
      await f.host.handleIntent(intent);
      expect(warning).toHaveBeenLastCalledWith("Unable to open diff /workspace/a.ts: unexpected non-Error");
      expect(f.getSessionFiles).toHaveBeenCalledTimes(1);
    }finally{warning.mockRestore();f.provider.dispose();}
  });
  it("validates file intents through the real whitelist",()=>{
    expect(isWebviewIntent({messageId:"x",type:"openSessionFileDiff",data:{sessionId:"s",sourceTurnId:"u",path:"/a"}})).toBe(true);
    expect(isWebviewIntent({messageId:"x",type:"restoreSessionFiles",data:{sessionId:"s",sourceTurnId:"u",paths:[1],requestId:"r"}})).toBe(false);
  });
});
