import { describe, expect, it } from "vitest";

import { parsePlanDocument } from "../planDocument";

const RUNTIME_PLAN = `---
plan_id: plan-abc123
goal: Build the plan preview custom editor
state: planning
session_id: null
created_at: 2026-01-01T00:00:00Z
schema_version: 1
todos:
- id: t1
  content: First task
  status: pending
- id: t2
  content: Second task
  status: in_progress
- id: t3
  content: Third task
  status: completed
- id: t4
  content: Fourth task
  status: cancelled
---

# Heading

Body paragraph.
`;

describe("parsePlanDocument", () => {
  it("parses runtime frontmatter with four todo states, planId and state", () => {
    const parsed = parsePlanDocument(RUNTIME_PLAN);
    expect(parsed.title).toBe("Build the plan preview custom editor");
    expect(parsed.planId).toBe("plan-abc123");
    expect(parsed.state).toBe("planning");
    expect(parsed.todos).toEqual([
      { content: "First task", id: "t1", status: "pending" },
      { content: "Second task", id: "t2", status: "in_progress" },
      { content: "Third task", id: "t3", status: "completed" },
      { content: "Fourth task", id: "t4", status: "cancelled" },
    ]);
    expect(parsed.bodyMarkdown).toContain("# Heading");
    expect(parsed.bodyMarkdown).toContain("Body paragraph.");
  });

  it("returns empty metadata and full body when there is no frontmatter", () => {
    const parsed = parsePlanDocument("# just a body\nno frontmatter here");
    expect(parsed.title).toBeNull();
    expect(parsed.overview).toBeNull();
    expect(parsed.planId).toBeNull();
    expect(parsed.state).toBeNull();
    expect(parsed.todos).toEqual([]);
    expect(parsed.bodyMarkdown).toContain("# just a body");
  });

  it("prefers name over goal, and falls back to goal (truncated) for title", () => {
    const byName = parsePlanDocument(`---
name: Named Plan
goal: some goal
---
`);
    expect(byName.title).toBe("Named Plan");

    const byGoal = parsePlanDocument(`---
goal: ${"目标".repeat(60)}
---
`);
    expect(byGoal.title).not.toBeNull();
    expect(byGoal.title!.length).toBeLessThanOrEqual(96);
    expect(byGoal.title!.endsWith("...")).toBe(true);
  });

  it("extracts the overview scalar", () => {
    const parsed = parsePlanDocument(`---
name: Demo
overview: Render the transcript UI with plan metadata.
---
# body
`);
    expect(parsed.overview).toBe("Render the transcript UI with plan metadata.");
  });

  it("tolerates the flat doc-style todos (content/status at column 0)", () => {
    const parsed = parsePlanDocument(`---
name: Doc Plan
todos:

- id: alpha
content: 认领任务：与 develop 同步
status: pending
- id: beta
content: 阅读上下文, 约束: 边界语义
status: in_progress
---
# body
`);
    expect(parsed.todos).toEqual([
      { content: "认领任务：与 develop 同步", id: "alpha", status: "pending" },
      { content: "阅读上下文, 约束: 边界语义", id: "beta", status: "in_progress" },
    ]);
  });

  it("handles empty todos and a sibling key after the todos block", () => {
    const parsed = parsePlanDocument(`---
name: Empty
todos: []
state: pending
---
# body
`);
    expect(parsed.todos).toEqual([]);
    expect(parsed.state).toBe("pending");
  });

  it("strips the auto-maintained Todos Board section from the body", () => {
    const parsed = parsePlanDocument(`---
name: Board Plan
todos:
- id: t1
  content: First
  status: pending
---
# My Plan

Intro paragraph.

## Todos Board

<!-- todos-board:auto:begin -->
### Todos
- [ ] t1: First
<!-- todos-board:auto:end -->

## Next Section

More text.
`);
    expect(parsed.bodyMarkdown).toContain("# My Plan");
    expect(parsed.bodyMarkdown).toContain("## Next Section");
    expect(parsed.bodyMarkdown).toContain("More text.");
    expect(parsed.bodyMarkdown).not.toContain("Todos Board");
    expect(parsed.bodyMarkdown).not.toContain("todos-board:auto");
    expect(parsed.bodyMarkdown).not.toContain("### Todos");
  });

  it("parses CRLF documents identically", () => {
    const parsed = parsePlanDocument(RUNTIME_PLAN.replace(/\n/g, "\r\n"));
    expect(parsed.title).toBe("Build the plan preview custom editor");
    expect(parsed.todos).toHaveLength(4);
    expect(parsed.todos[1]).toEqual({
      content: "Second task",
      id: "t2",
      status: "in_progress",
    });
    expect(parsed.bodyMarkdown).toContain("# Heading");
  });

  it("keeps the raw source verbatim for the Markdown source view", () => {
    const parsed = parsePlanDocument(RUNTIME_PLAN);
    expect(parsed.raw).toBe(RUNTIME_PLAN);
  });

  const lineOf = (parsed: { bodyLineMap: number[]; bodyMarkdown: string }, needle: string): number => {
    const index = parsed.bodyMarkdown.split("\n").indexOf(needle);
    return parsed.bodyLineMap[index];
  };

  it("maps body lines to absolute file lines across the frontmatter offset", () => {
    const parsed = parsePlanDocument(RUNTIME_PLAN);
    expect(parsed.bodyLineMap).toHaveLength(parsed.bodyMarkdown.split("\n").length);
    // `# Heading` is the 23rd line of RUNTIME_PLAN, `Body paragraph.` the 25th.
    expect(lineOf(parsed, "# Heading")).toBe(23);
    expect(lineOf(parsed, "Body paragraph.")).toBe(25);
  });

  it("maps body lines starting at line 1 when there is no frontmatter", () => {
    const parsed = parsePlanDocument("# just a body\nno frontmatter here");
    expect(parsed.bodyLineMap[0]).toBe(1);
    expect(lineOf(parsed, "no frontmatter here")).toBe(2);
  });

  it("maps lines non-linearly around a spliced-out Todos Board", () => {
    const parsed = parsePlanDocument(`---
name: Board Plan
todos:
- id: t1
  content: First
  status: pending
---
# My Plan

Intro paragraph.

## Todos Board

<!-- todos-board:auto:begin -->
### Todos
- [ ] t1: First
<!-- todos-board:auto:end -->

## Next Section

More text.
`);
    expect(lineOf(parsed, "# My Plan")).toBe(8);
    expect(lineOf(parsed, "Intro paragraph.")).toBe(10);
    // The board occupied lines 12-17; `## Next Section` must jump to its real
    // source line (19), not the linear body offset.
    expect(lineOf(parsed, "## Next Section")).toBe(19);
    expect(lineOf(parsed, "More text.")).toBe(21);
  });

  it("keeps line mapping identical for CRLF documents", () => {
    const parsed = parsePlanDocument(RUNTIME_PLAN.replace(/\n/g, "\r\n"));
    expect(lineOf(parsed, "# Heading")).toBe(23);
    expect(lineOf(parsed, "Body paragraph.")).toBe(25);
  });
});
