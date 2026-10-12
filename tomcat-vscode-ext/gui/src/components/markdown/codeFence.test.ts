import { describe, expect, it } from "vitest";

import { parseCodeFenceInfo } from "./codeFence";

describe("parseCodeFenceInfo", () => {
  it("parses explicit language + file path", () => {
    expect(parseCodeFenceInfo("rust src/core/foo.rs")).toMatchObject({
      filePath: "src/core/foo.rs",
      isMermaid: false,
      language: "rust",
      languageLabel: "rust",
      line: undefined,
    });
  });

  it("parses explicit language + file path with line", () => {
    expect(parseCodeFenceInfo("rust src/core/foo.rs:42")).toMatchObject({
      filePath: "src/core/foo.rs",
      isMermaid: false,
      language: "rust",
      languageLabel: "rust",
      line: 42,
    });
  });

  it("infers a language when the fence only contains a file path", () => {
    expect(parseCodeFenceInfo("src/gui/App.tsx")).toMatchObject({
      filePath: "src/gui/App.tsx",
      isMermaid: false,
      language: "typescript",
      languageLabel: "typescript",
    });
  });

  it("keeps language-only fences as plain code cards", () => {
    const parsed = parseCodeFenceInfo("rust");
    expect(parsed).toMatchObject({
      isMermaid: false,
      language: "rust",
      languageLabel: "rust",
    });
    expect(parsed.filePath).toBeUndefined();
  });

  it("routes mermaid fences to mermaid rendering", () => {
    const parsed = parseCodeFenceInfo("mermaid");
    expect(parsed).toMatchObject({
      isMermaid: true,
      language: "mermaid",
      languageLabel: "mermaid",
    });
    expect(parsed.filePath).toBeUndefined();
  });
});
