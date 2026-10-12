import { describe, expect, it } from "vitest";

import { classifyLink, splitPathLocation } from "../linkTarget";

describe("linkTarget", () => {
  it("keeps external links and ignores empty or local anchors", () => {
    expect(classifyLink("https://example.com/docs")).toEqual({
      href: "https://example.com/docs",
      kind: "external",
    });
    expect(classifyLink("mailto:hi@example.com")).toEqual({
      href: "mailto:hi@example.com",
      kind: "external",
    });
    expect(classifyLink("#section")).toEqual({ kind: "ignore" });
  });

  it("strips supported source locations while retaining the first line", () => {
    expect(splitPathLocation("docs/design.md:42")).toEqual({
      line: 42,
      path: "docs/design.md",
    });
    expect(splitPathLocation("src/App.tsx:59-103")).toEqual({
      line: 59,
      path: "src/App.tsx",
    });
    expect(splitPathLocation("src/lib.rs#L9-L20")).toEqual({
      line: 9,
      path: "src/lib.rs",
    });
  });

  it.each(["src/app.ts", "C:\\work\\app.ts"])("separates line and column from %s", (path) => {
    for (const suffix of [":42:7", ":42:7-50:9"]) {
      expect(splitPathLocation(`${path}${suffix}`)).toEqual({ path, line: 42 });
      expect(classifyLink(`${path}${suffix}`)).toEqual({ kind: "file", path, line: 42 });
    }
  });

  it("strips a regular local Markdown anchor without treating it as a line", () => {
    expect(classifyLink("docs/design.md#overview")).toEqual({
      kind: "file",
      path: "docs/design.md",
    });
  });
});
