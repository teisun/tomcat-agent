import * as assert from "node:assert/strict";

/** A denied write must remain an error after reload; no release-version exception. */
export function assertDeniedHistoryStatus(isError: unknown): void {
  assert.equal(isError, true, "A denied write must retain its error status after history reload");
}
