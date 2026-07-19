import { describe, expect, it } from "vitest";

import { highlightToHtml } from "./richRenderRuntime";

describe("highlightToHtml", () => {
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
