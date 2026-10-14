import { describe, expect, it } from "vitest";
import { deniedHistoryFinding } from "../e2e-harness/src/test/release-bundle-findings";

describe("native release known finding boundary", () => {
  it.each(["0.1.73", "0.1.74"])("reports the specifically accepted CLI %s history label defect", (version) => {
    const finding = deniedHistoryFinding(false, version);
    expect(finding).toContain(`CLI ${version}:`);
    expect(finding).toContain("known issue, not a fixed or visually passing result");
  });

  it("does not report the defect when the restored denied status is correct", () => {
    expect(deniedHistoryFinding(true, "0.1.73")).toBeUndefined();
    expect(deniedHistoryFinding(true, "0.1.74")).toBeUndefined();
  });

  it("does not extend the exception to any different CLI pin", () => {
    expect(() => deniedHistoryFinding(false, "0.1.75")).toThrow(/different CLI version/);
    expect(() => deniedHistoryFinding(false, "")).toThrow(/different CLI version/);
  });

  it.each(["0.1.73", "0.1.74"])("rejects a missing tool status for CLI %s instead of hiding a lost history record", (version) => {
    expect(() => deniedHistoryFinding(undefined, version)).toThrow(/explicit history status/);
    expect(() => deniedHistoryFinding(null, version)).toThrow(/explicit history status/);
  });
});
