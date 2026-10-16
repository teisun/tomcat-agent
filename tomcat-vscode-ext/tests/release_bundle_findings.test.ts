import { describe, expect, it } from "vitest";
import { assertDeniedHistoryStatus } from "../e2e-harness/src/test/release-bundle-findings";

describe("native release denied history boundary", () => {
  it("accepts the restored error status", () => {
    expect(() => assertDeniedHistoryStatus(true)).not.toThrow();
  });

  it.each([false, undefined, null, "true"])("rejects missing or incorrect status %s without a version exception", (status) => {
    expect(() => assertDeniedHistoryStatus(status)).toThrow(/retain its error status/);
  });
});
