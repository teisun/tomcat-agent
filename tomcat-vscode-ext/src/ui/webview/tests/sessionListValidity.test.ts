import { describe, expect, it, vi } from "vitest";
import { SessionRouter } from "../../../serveClient/sessionRouter";

describe("session list absence evidence", () => {
  it.each([
    { success: false, error: "not_initialized", payload: { sessions: [] } },
    { success: true, payload: null },
    { success: true, payload: { sessions: [null] } },
    { success: true, payload: { sessions: [{ sessionId: "" }] } },
    { success: true, payload: { sessions: "incomplete" } },
  ])("does not turn a failed or incomplete response into an empty disk list: %j", async response => {
    const router = new SessionRouter({ request: vi.fn().mockResolvedValue(response) } as never, () => "/workspace");
    await expect(router.listSessions("disk")).rejects.toThrow();
  });
  it("carries a valid empty disk list's scope identity", async () => {
    const router = new SessionRouter({ request: vi.fn().mockResolvedValue({ success: true, payload: { sessions: [], sessionKey: "scope-A" } }) } as never, () => "/workspace");
    expect(await router.listSessions("disk")).toMatchObject({ scope: "disk", sessionKey: "scope-A", sessions: [] });
  });
});
