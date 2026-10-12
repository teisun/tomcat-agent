import { describe, expect, it } from "vitest";

import type { FileDiffLine } from "../../serveClient/wire";
import {
  createDiffPresentation,
  isDiffViewable,
} from "../diffPresentation";

const change: FileDiffLine[] = [
  { tag: "ctx", text: "function setup() {", oldLine: 25, newLine: 25 },
  { tag: "del", text: "  return oldValue;", oldLine: 26 },
  { tag: "add", text: "  return newValue;", newLine: 26 },
  { tag: "ctx", text: "}", oldLine: 27, newLine: 27 },
];
const gap: FileDiffLine = { tag: "gap", text: "omitted", skippedLines: 100 };

describe("diff availability", () => {
  it("allows changes even when unchanged context has been omitted", () => {
    expect(isDiffViewable({ diff: change })).toBe(true);
    expect(isDiffViewable({ diff: [gap, ...change, gap] })).toBe(true);
  });

  it("rejects unavailable records and records with no changes", () => {
    expect(isDiffViewable({ diff: undefined })).toBe(false);
    expect(isDiffViewable({ diff: null })).toBe(false);
    expect(isDiffViewable({ diff: [] })).toBe(false);
    expect(isDiffViewable({ diff: [change[0], gap] })).toBe(false);
    expect(isDiffViewable({ diff: change, diffTruncated: true })).toBe(false);
    expect(isDiffViewable({ diff: undefined, diffTruncated: true })).toBe(false);
    expect(isDiffViewable({ diff: change, diffTruncated: true, diffExpired: true })).toBe(false);
    expect(isDiffViewable({ diff: change, diffExpired: true })).toBe(false);
  });
});

describe("saved diff presentation", () => {
  it("projects a complete diff preserving logical line text and context", () => {
    expect(createDiffPresentation(change)).toEqual({
      kind: "full",
      fragments: [{
        before: "function setup() {\n  return oldValue;\n}\n",
        after: "function setup() {\n  return newValue;\n}\n",
        oldRange: { start: 25, end: 27 },
        newRange: { start: 25, end: 27 },
      }],
    });
  });

  it("keeps a single change fragment when gaps surround it", () => {
    const presentation = createDiffPresentation([gap, ...change, gap]);
    expect(presentation.kind).toBe("fragments");
    expect(presentation.fragments).toEqual(createDiffPresentation(change).fragments);
  });

  it("splits distant repeated code so the editor cannot match across gaps", () => {
    const first: FileDiffLine[] = [
      { tag: "del", text: "function alpha() {}", oldLine: 30 },
      { tag: "add", text: "function beta() {}", newLine: 30 },
    ];
    const second: FileDiffLine[] = [
      { tag: "del", text: "function beta() {}", oldLine: 131 },
      { tag: "add", text: "function alpha() {}", newLine: 131 },
    ];
    expect(createDiffPresentation([...first, gap, ...second])).toEqual({
      kind: "fragments",
      fragments: [
        { before: "function alpha() {}\n", after: "function beta() {}\n", oldRange: { start: 30, end: 30 }, newRange: { start: 30, end: 30 } },
        { before: "function beta() {}\n", after: "function alpha() {}\n", oldRange: { start: 131, end: 131 }, newRange: { start: 131, end: 131 } },
      ],
    });
  });

  it("drops groups without changes", () => {
    const legacyGap: FileDiffLine = { tag: "gap", text: "legacy marker" };
    const presentation = createDiffPresentation([
      { tag: "ctx", text: "unchanged", oldLine: 1, newLine: 1 },
      legacyGap,
      legacyGap,
      ...change,
      legacyGap,
    ]);
    expect(presentation.kind).toBe("fragments");
    expect(presentation.fragments).toEqual(createDiffPresentation(change).fragments);
  });

  it("preserves empty sides of pure insertions and deletions", () => {
    expect(createDiffPresentation([
      { tag: "add", text: "new line", newLine: 1 },
      gap,
      { tag: "del", text: "old line", oldLine: 103 },
    ]).fragments).toEqual([
      { before: "", after: "new line\n", oldRange: undefined, newRange: { start: 1, end: 1 } },
      { before: "old line\n", after: "", oldRange: { start: 103, end: 103 }, newRange: undefined },
    ]);
  });

  it.each(["add", "del"] as const)("preserves a single empty logical line for %s", (tag) => {
    const presentation = createDiffPresentation([{ tag, text: "" }]);
    expect(presentation.fragments[0]).toMatchObject(
      tag === "add" ? { before: "", after: "\n" } : { before: "\n", after: "" },
    );
  });

  it("preserves repeated empty lines on both sides of a change", () => {
    const presentation = createDiffPresentation([
      { tag: "del", text: "" },
      { tag: "del", text: "" },
      { tag: "add", text: "" },
      { tag: "add", text: "" },
      { tag: "add", text: "" },
    ]);
    expect(presentation.fragments[0]).toMatchObject({ before: "\n\n", after: "\n\n\n" });
  });

  it("terminates both sides consistently without inferring an unsaved EOF newline", () => {
    const presentation = createDiffPresentation([
      { tag: "ctx", text: "  context  " },
      { tag: "del", text: "old" },
      { tag: "add", text: "new" },
    ]);
    expect(presentation.fragments[0]).toMatchObject({
      before: "  context  \nold\n",
      after: "  context  \nnew\n",
    });
  });

  it("does not infer source ranges when saved line numbers are incomplete", () => {
    const presentation = createDiffPresentation([
      { tag: "ctx", text: "context", oldLine: 1, newLine: 1 },
      { tag: "del", text: "old" },
      { tag: "add", text: "new", newLine: 2 },
    ]);
    expect(presentation.fragments[0].oldRange).toBeUndefined();
    expect(presentation.fragments[0].newRange).toEqual({ start: 1, end: 2 });
  });

  it("does not create a preview for empty or unchanged records", () => {
    expect(createDiffPresentation([])).toEqual({ kind: "full", fragments: [] });
    expect(createDiffPresentation([change[0], gap])).toEqual({ kind: "fragments", fragments: [] });
  });
});
