import { describe, expect, it } from "vitest";

import {
  detectInlineFilePath,
  inferLanguageFromPath,
  looksLikeFilePathToken,
  splitInlinePathLocation,
} from "./inlinePath";

describe("inlinePath helpers", () => {
  it("accepts relative and absolute-looking file paths", () => {
    expect(looksLikeFilePathToken("src/app.ts")).toBe(true);
    expect(looksLikeFilePathToken("./src/app.ts")).toBe(true);
    expect(looksLikeFilePathToken("/workspace/src/app.ts")).toBe(true);
  });

  it("rejects urls and whitespace-heavy text", () => {
    expect(looksLikeFilePathToken("https://example.com")).toBe(false);
    expect(looksLikeFilePathToken("foo bar")).toBe(false);
    expect(looksLikeFilePathToken("plain text")).toBe(false);
  });

  it("parses colon line suffixes", () => {
    expect(splitInlinePathLocation("src/app.ts:42")).toEqual({
      line: 42,
      originalText: "src/app.ts:42",
      path: "src/app.ts",
    });
  });

  it("parses hash-style line suffixes", () => {
    expect(splitInlinePathLocation("a.rs#L9")).toEqual({
      line: 9,
      originalText: "a.rs#L9",
      path: "a.rs",
    });
  });

  it("detects clickable inline file paths", () => {
    expect(detectInlineFilePath("gui/App.tsx")).toMatchObject({
      originalText: "gui/App.tsx",
      path: "gui/App.tsx",
    });
  });

  it("infers highlight languages from extensions", () => {
    expect(inferLanguageFromPath("src/app.ts")).toBe("typescript");
    expect(inferLanguageFromPath("src/lib.rs")).toBe("rust");
    expect(inferLanguageFromPath("README.md")).toBe("markdown");
  });
});
