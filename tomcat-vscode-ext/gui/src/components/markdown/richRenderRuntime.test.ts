import { describe, expect, it, vi } from "vitest";

vi.mock("mermaid", () => { throw new Error("mermaid unavailable"); });

import { getMermaid, highlightToHtml, warmRichRenderModules } from "./richRenderRuntime";

describe("highlightToHtml", () => {
  it("keeps optional warmup failures local while on-demand failures remain observable", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      await expect(warmRichRenderModules()).resolves.toBeUndefined();
      expect(warn).toHaveBeenCalledWith(expect.stringContaining("warmup: FAIL"));
      await expect(getMermaid()).rejects.toThrow();
      expect(highlightToHtml("const answer = 42;", "typescript").language).toBe("typescript");
    } finally {
      warn.mockRestore();
    }
  });

  it("highlights registered languages synchronously", () => {
    const result = highlightToHtml("const answer = 42;\n", "typescript");

    expect(result.language).toBe("typescript");
    expect(result.html).toMatch(/hljs-(keyword|number)/u);
  });

  it("accepts registered aliases without awaiting module warmup", () => {
    const result = highlightToHtml("const answer: number = 42;\n", "ts");

    expect(result.language).not.toBe("plaintext");
    expect(result.html).toMatch(/hljs-(keyword|number|built_in|type)/u);
  });

  it("falls back to plaintext for unknown languages", () => {
    const result = highlightToHtml("plain <unsafe>\n", "definitely-not-a-language");

    expect(result.language).toBe("plaintext");
    expect(result.html).toContain("&lt;unsafe&gt;");
  });
});
