import { describe, expect, it, vi } from "vitest";

import { SessionRouter } from "../sessionRouter";

describe("SessionRouter session files", () => {
  const payload = {sessionId:"s",sourceTurnId:"u",files:[{path:"/a",status:"modified",added:1,removed:1,restorable:true}]};
  it("validates list identity and entries rather than silently dropping invalid files", async () => {
    const messenger={request:vi.fn().mockResolvedValue({sessionId:"s",success:true,payload})};
    const router=new SessionRouter(messenger as never,()=>undefined);
    await expect(router.getSessionFiles("s")).resolves.toEqual(payload);
    expect(messenger.request).toHaveBeenCalledWith({type:"get_session_files",sessionId:"s"});
    messenger.request.mockResolvedValueOnce({success:true,sessionId:"other",payload});
    await expect(router.getSessionFiles("s")).rejects.toThrow("mismatch");
    messenger.request.mockResolvedValueOnce({success:true,sessionId:"s",payload:{...payload,files:[{...payload.files[0],added:-1}]}});
    await expect(router.getSessionFiles("s")).rejects.toThrow("entry");
  });
  it("keeps against the current source and validates the new source and session", async () => {
    const messenger = { request: vi.fn().mockResolvedValue({ sessionId: "s", success: true, payload: { sessionId: "s", sourceTurnId: "keep-1" } }) };
    const router = new SessionRouter(messenger as never, () => undefined);
    await expect(router.keepSessionFiles("s", "u")).resolves.toEqual({ sessionId: "s", sourceTurnId: "keep-1" });
    expect(messenger.request).toHaveBeenCalledWith({ type: "keep_session_files", sessionId: "s", sourceTurnId: "u" });
    for (const sourceTurnId of [null, "", "u"]) {
      messenger.request.mockResolvedValueOnce({ sessionId: "s", success: true, payload: { sessionId: "s", sourceTurnId } });
      await expect(router.keepSessionFiles("s", "u")).rejects.toThrow("Invalid");
    }
    messenger.request.mockResolvedValueOnce({ sessionId: "other", success: true, payload: { sessionId: "s", sourceTurnId: "keep-2" } });
    await expect(router.keepSessionFiles("s", "u")).rejects.toThrow("mismatch");
    messenger.request.mockResolvedValueOnce({ sessionId: "s", success: true, payload: { ...payload, files: [{ ...payload.files[0], blockedReason: "head_moved" }] } });
    await expect(router.getSessionFiles("s")).rejects.toThrow("entry");
  });
  it("binds baseline and restore payloads to the exact turn/path", async () => {
    const messenger={request:vi.fn().mockResolvedValue({sessionId:"s",success:true,payload:{sessionId:"s",sourceTurnId:"u",path:"/a",existed:true,text:"before"}})};
    const router=new SessionRouter(messenger as never,()=>undefined);
    expect((await router.getSessionFileBaseline("s","u","/a")).text).toBe("before");
    expect(messenger.request).toHaveBeenCalledWith({type:"get_session_file_baseline",sessionId:"s",sourceTurnId:"u",path:"/a"});
    messenger.request.mockResolvedValueOnce({sessionId:"s",success:true,payload:{sessionId:"s",sourceTurnId:"wrong",restored:["/a"]}});
    await expect(router.restoreSessionFiles("s","u",["/a"])).rejects.toThrow("Invalid");
    messenger.request.mockResolvedValueOnce({sessionId:"s",success:false,error:"restore_failed",payload:{path:"/a",reason:"disk full"}});
    await expect(router.restoreSessionFiles("s","u",["/a"])).rejects.toThrow("restore_failed: /a: disk full");
  });
});

describe("SessionRouter shared slash commands", () => {
  it("sends original text and session identity with a ten-minute wait", async () => {
    const messenger = {request:vi.fn().mockResolvedValue({success:true, payload:{ok:true, text:"done"}})};
    const router = new SessionRouter(messenger as never, () => "/workspace");
    const text = "  /install './folder with space' agent\n";
    await expect(router.runSlashCommand("s1", text)).resolves.toEqual({ok:true, text:"done"});
    expect(messenger.request).toHaveBeenCalledWith({type:"run_slash_command", sessionId:"s1", text}, 600_000);
  });
  it("preserves usage errors and rejects invalid payloads or a transport-level busy", async () => {
    const text = "用法";
    const messenger = {request:vi.fn().mockResolvedValue({success:true, payload:{ok:false, text}})};
    const router = new SessionRouter(messenger as never, () => undefined);
    await expect(router.runSlashCommand("s1", "/install")).resolves.toEqual({ok:false, text});
    messenger.request.mockResolvedValueOnce({success:true, payload:{ok:"true", text:"bad"}});
    await expect(router.runSlashCommand("s1", "/reload")).rejects.toThrow("payload is invalid");
    messenger.request.mockResolvedValueOnce({success:false, error:"busy"});
    await expect(router.runSlashCommand("s1", "/reload")).rejects.toThrow("busy");
  });
});

describe("SessionRouter checkpoint methods", () => {
  it("parses listCheckpoints payloads", async () => {
    const messenger = {
      request: vi.fn().mockResolvedValue({
        payload: {
          checkpoints: [
            {
              changedFiles: ["src/app.ts"],
              createdAt: "2026-07-12T12:00:00Z",
              id: "ck-1",
              kind: "turn_end",
              label: null,
              messageAnchor: "assistant-1",
              sessionId: "s1",
            },
          ],
          sessionId: "s1",
        },
        success: true,
      }),
    };
    const router = new SessionRouter(messenger as never, () => "/workspace");

    await expect(router.listCheckpoints("s1")).resolves.toEqual({
      checkpoints: [
        {
          changedFiles: ["src/app.ts"],
          createdAt: "2026-07-12T12:00:00Z",
          id: "ck-1",
          kind: "turn_end",
          label: null,
          messageAnchor: "assistant-1",
        },
      ],
      sessionId: "s1",
    });
    expect(messenger.request).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: "s1",
        type: "list_checkpoints",
      }),
    );
  });

  it("parses restoreCheckpoint payloads with revertFiles", async () => {
    const messenger = {
      request: vi.fn().mockResolvedValue({
        payload: {
          changedPaths: ["src/app.ts"],
          checkpointId: "ck-2",
          createdAt: "2026-07-12T12:05:00Z",
          dryRun: false,
          kind: "turn_end",
          label: "after edit",
          messageAnchor: "assistant-2",
          reloadedPlanId: "plan-1",
          restoredPaths: ["src/app.ts"],
          revertFiles: false,
          sessionId: "s2",
          summary: " src/app.ts | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n",
          transcriptTruncated: true,
          warnings: ["other sessions also changed this file"],
        },
        success: true,
      }),
    };
    const router = new SessionRouter(messenger as never, () => "/workspace");

    await expect(router.restoreCheckpoint("s2", "ck-2", false)).resolves.toEqual({
      changedPaths: ["src/app.ts"],
      checkpointId: "ck-2",
      createdAt: "2026-07-12T12:05:00Z",
      dryRun: false,
      kind: "turn_end",
      label: "after edit",
      messageAnchor: "assistant-2",
      reloadedPlanId: "plan-1",
      restoredPaths: ["src/app.ts"],
      revertFiles: false,
      sessionId: "s2",
      summary: " src/app.ts | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n",
      transcriptTruncated: true,
      warnings: ["other sessions also changed this file"],
    });
    expect(messenger.request).toHaveBeenCalledWith(
      expect.objectContaining({
        checkpointId: "ck-2",
        revertFiles: false,
        sessionId: "s2",
        type: "restore_checkpoint",
      }),
    );
  });

  it("sends compact and validates its usage report", async () => {
    const messenger = {
      request: vi.fn().mockResolvedValue({
        payload: {
          afterUsageRatio: 0.21,
          beforeUsageRatio: 0.93,
          coveredMessageCount: 42,
        },
        success: true,
      }),
    };
    const router = new SessionRouter(messenger as never, () => "/workspace");

    await expect(router.compact("s1")).resolves.toEqual({
      afterUsageRatio: 0.21,
      beforeUsageRatio: 0.93,
      coveredMessageCount: 42,
    });
    expect(messenger.request).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "s1", type: "compact" }),
    );
  });
});
