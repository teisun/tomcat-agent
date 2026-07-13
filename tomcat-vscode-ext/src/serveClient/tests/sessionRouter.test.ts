import { describe, expect, it, vi } from "vitest";

import { SessionRouter } from "../sessionRouter";

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
});
