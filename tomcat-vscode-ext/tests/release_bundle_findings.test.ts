import { describe, expect, it } from "vitest";
import { deniedHistoryFinding } from "../e2e-harness/src/test/release-bundle-findings";

describe("native release known finding boundary", () => {
  it("reports the specifically accepted CLI 0.1.73 history label defect", () => {
    expect(deniedHistoryFinding(false, "0.1.73")).toContain("known issue, not a fixed or visually passing result");
  });

  it("does not report the defect when the restored denied status is correct", () => {
    expect(deniedHistoryFinding(true, "0.1.73")).toBeUndefined();
    expect(deniedHistoryFinding(true, "0.1.74")).toBeUndefined();
  });

  it("does not extend the exception to any different CLI pin", () => {
    expect(() => deniedHistoryFinding(false, "0.1.74")).toThrow(/different CLI version/);
    expect(() => deniedHistoryFinding(false, "")).toThrow(/different CLI version/);
  });

  it("rejects a missing tool status instead of hiding a lost history record", () => {
    expect(() => deniedHistoryFinding(undefined, "0.1.73")).toThrow(/explicit history status/);
    expect(() => deniedHistoryFinding(null, "0.1.73")).toThrow(/explicit history status/);
  });
});
