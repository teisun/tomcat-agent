import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { WebviewReviewRow } from "../types";
import { ReviewRow } from "./ReviewRow";

function buildReview(overrides: Partial<WebviewReviewRow> = {}): WebviewReviewRow {
  return {
    findings: [{ area: "logic", note: "Missing null guard", severity: "concern" }],
    id: "review:plan-1",
    planId: "plan-1",
    rounds: 1,
    status: "done",
    summary: "Fix the missing null guard before completing the plan.",
    type: "review",
    verdict: "partial",
    ...overrides,
  };
}

describe("ReviewRow", () => {
  it("renders a shimmering running row while code review is in progress", () => {
    render(<ReviewRow item={buildReview({ findings: undefined, status: "running", summary: null })} />);

    expect(screen.getByTestId("review-row-running-text").textContent).toBe("Reviewing code...");
    expect(screen.getByTestId("review-row-running-text").className).toContain("tc-loading-shimmer");
    expect(screen.queryByTestId("review-row-toggle")).toBeNull();
  });

  it("renders verdict badge, rounds and findings for completed reviews", () => {
    render(<ReviewRow item={buildReview()} />);

    expect(screen.getByTestId("review-row-verdict").textContent).toBe("PARTIAL");
    expect(screen.getByTestId("review-row-findings-count").textContent).toBe("1 finding");
    expect(screen.getByTestId("review-row-preview").textContent).toContain(
      "Fix the missing null guard",
    );

    fireEvent.click(screen.getByTestId("review-row-toggle"));
    expect(screen.getByTestId("review-row-rounds").textContent).toBe("Review round 1");
    expect(screen.getByTestId("review-row-summary").textContent).toContain(
      "Fix the missing null guard",
    );
    expect(screen.getByTestId("review-row-findings").textContent).toContain("Missing null guard");
    expect(screen.getByTestId("review-row-findings").textContent).toContain("concern");
  });

  it("falls back to empty-findings copy when no structured findings are returned", () => {
    render(
      <ReviewRow
        item={buildReview({
          findings: [],
          summary: null,
          verdict: "pass",
        })}
      />,
    );

    expect(screen.getByTestId("review-row-verdict").textContent).toBe("PASS");
    expect(screen.getByTestId("review-row-preview").textContent).toBe(
      "Expand to inspect review details.",
    );

    fireEvent.click(screen.getByTestId("review-row-toggle"));
    expect(screen.getByTestId("review-row-empty-findings").textContent).toBe(
      "No structured findings were returned.",
    );
  });
});
