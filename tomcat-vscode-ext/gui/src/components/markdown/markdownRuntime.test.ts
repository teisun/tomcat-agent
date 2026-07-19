import { describe, expect, it } from "vitest";

import { splitTopLevelBlocks } from "./markdownRuntime";

describe("splitTopLevelBlocks", () => {
  it("splits top-level markdown into raw non-space blocks", () => {
    const blocks = splitTopLevelBlocks(
      "# Title\n\nParagraph.\n\n- one\n- two\n\n```ts\nconst answer = 42;\n```\n",
    );

    expect(blocks.map((block) => block.trim())).toEqual([
      "# Title",
      "Paragraph.",
      "- one\n- two",
      "```ts\nconst answer = 42;\n```",
    ]);
  });

  it("filters blank space tokens instead of rendering empty blocks", () => {
    const blocks = splitTopLevelBlocks("\n\nParagraph one.\n\n\nParagraph two.\n\n");

    expect(blocks).toHaveLength(2);
    expect(blocks.every((block) => block.trim().length > 0)).toBe(true);
  });

  it("keeps an unterminated fence inside the tail block", () => {
    const blocks = splitTopLevelBlocks("Intro\n\n```ts\nconst answer = 42;");

    expect(blocks).toHaveLength(2);
    expect(blocks[0].trim()).toBe("Intro");
    expect(blocks[1]).toBe("```ts\nconst answer = 42;");
  });
});
