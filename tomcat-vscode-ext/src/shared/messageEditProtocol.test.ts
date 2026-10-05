import { describe, expect, it } from "vitest";
import { isPreviewRewindResponse, rewindErrorDetail } from "./messageEditProtocol";

describe("message edit result contracts", () => {
  it("retains a partial restore's path and cause in user-facing feedback", () => {
    expect(rewindErrorDetail("revert_failed", {path:"src/a.ts",reason:"permission denied"})).toContain("src/a.ts");
    expect(rewindErrorDetail("revert_failed", {path:"src/a.ts",reason:"permission denied"})).toContain("permission denied");
    expect(rewindErrorDetail("revert_failed", {path:3})).toBeUndefined();
  });
  it("does not claim a timed-out task has stopped", () => {
    expect(rewindErrorDetail("stop_timeout",null)).toContain("停止当前任务超时");
  });
  it("validates preview paths and availability reasons", () => {
    expect(isPreviewRewindResponse({revertAvailable:false,revertPaths:[],revertReason:"expired"})).toBe(true);
    expect(isPreviewRewindResponse({revertAvailable:true,revertPaths:[3]})).toBe(false);
    expect(isPreviewRewindResponse({revertAvailable:false,revertPaths:[],revertReason:"unknown"})).toBe(false);
  });
});
