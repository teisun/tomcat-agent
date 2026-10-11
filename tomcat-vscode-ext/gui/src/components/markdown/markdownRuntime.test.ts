import { describe, expect, it } from "vitest";

import { renderMarkdownBlocks } from "./markdownRuntime";

describe("renderMarkdownBlocks", () => {
  it("renders top-level markdown as non-space blocks with fence metadata", () => {
    const blocks = renderMarkdownBlocks(
      "# Title\n\nParagraph.\n\n- one\n- two\n\n```ts\nconst answer = 42;\n```\n",
    );

    expect(blocks).toHaveLength(4);
    expect(blocks[0]).toContain("<h1>Title</h1>");
    expect(blocks[1]).toContain("<p>Paragraph.</p>");
    expect(blocks[2]).toContain("<li>one</li>");
    expect(blocks[2]).toContain("<li>two</li>");
    expect(blocks[3]).toContain('data-fence-info="ts"');
    expect(blocks[3]).toContain("const answer = 42;");
  });

  it("filters blank space and non-rendering reference definitions", () => {
    const blocks = renderMarkdownBlocks("\n\nParagraph one.\n\n\nParagraph two.\n\n[unused]: https://example.com\n");
    expect(blocks).toHaveLength(2);
    expect(blocks.every((block) => block.trim().length > 0)).toBe(true);
  });

  it("renders an unterminated fence in the tail without inventing closing text", () => {
    const blocks = renderMarkdownBlocks("Intro\n\n```ts\nconst answer = 42;");
    expect(blocks).toHaveLength(2);
    expect(blocks[0].trim()).toBe("<p>Intro</p>");
    expect(blocks[1]).toContain('data-fence-info="ts"');
    expect(blocks[1]).toContain("const answer = 42;\n</code></pre>");
    expect(blocks[1]).not.toContain("```");
  });

  it("lets the parser distinguish a short inner fence from its long outer fence", () => {
    const blocks = renderMarkdownBlocks("````text\na\n```\nb");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toContain("a\n```\nb\n</code></pre>");
  });
});
