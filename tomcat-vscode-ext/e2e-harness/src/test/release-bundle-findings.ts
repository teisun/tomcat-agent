import * as assert from "node:assert/strict";

/** Only the release owner's explicitly deferred CLI 0.1.73/0.1.74 display defect is non-blocking. */
export function deniedHistoryFinding(isError: unknown, bundledCliVersion: string): string | undefined {
  assert.equal(typeof isError, "boolean", "The denied tool must exist and retain an explicit history status");
  if (isError === true) return undefined;
  assert.ok(["0.1.73", "0.1.74"].includes(bundledCliVersion), "A different CLI version must preserve denied-write error status after reload");
  return `CLI ${bundledCliVersion}: a denied write is incorrectly labelled Created after history reload. The file was not created; the release owner deferred this presentation repair. This is a known issue, not a fixed or visually passing result.`;
}
