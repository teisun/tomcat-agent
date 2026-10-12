import { describe, expect, it } from "vitest";

import { DRAFT_FORK_LIMITS, parseDraftForkCapture } from "./draftForkProtocol";

const valid = {
  cwd: "/workspace",
  operationId: "fork-1",
  segments: [
    { text: "hello", type: "text" as const },
    { kind: "file" as const, label: "a.ts", path: "/workspace/a.ts", type: "reference" as const },
  ],
  sourceSessionId: "source-1",
  text: "hello @a.ts",
};

describe("draft fork capture contract", () => {
  it("accepts and deep-copies a bounded click-time snapshot", () => {
    const parsed = parseDraftForkCapture(valid);
    expect(parsed).not.toBeNull();
    valid.segments[0] = { text: "changed", type: "text" };
    expect(parsed?.segments[0]).toEqual({ text: "hello", type: "text" });
  });

  it.each([
    ["missing operation id", { ...valid, operationId: "" }],
    ["oversized operation id", { ...valid, operationId: "x".repeat(DRAFT_FORK_LIMITS.operationIdBytes + 1) }],
    ["oversized text", { ...valid, text: "x".repeat(DRAFT_FORK_LIMITS.textBytes + 1) }],
    ["too many segments", { ...valid, segments: Array.from({ length: DRAFT_FORK_LIMITS.segmentCount + 1 }, () => ({ text: "x", type: "text" })) }],
    ["unknown segment", { ...valid, segments: [{ type: "image", bytes: "raw" }] }],
  ])("rejects %s", (_name, value) => {
    expect(parseDraftForkCapture(value)).toBeNull();
  });

  it("accepts selection references and retains their line metadata", () => {
    const parsed = parseDraftForkCapture({
      ...valid,
      segments: [{
        type: "reference",
        kind: "selection",
        label: "main.rs:1-2",
        path: "/repo/main.rs",
        lineStart: 1,
        lineEnd: 2,
        text: "fn main() {}",
      }],
    });
    expect(parsed?.segments[0]).toMatchObject({
      kind: "selection",
      lineStart: 1,
      lineEnd: 2,
      path: "/repo/main.rs",
    });
  });
});
