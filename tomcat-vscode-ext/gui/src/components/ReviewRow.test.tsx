import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WebviewReviewRow } from "../types";
import { ReviewRow } from "./ReviewRow";

function buildReview(overrides: Partial<WebviewReviewRow> = {}): WebviewReviewRow {
  return {
    findings: [{ area: "logic", note: "Missing null guard", severity: "concern" }],
    id: "review:plan-1",
    planId: "plan-1",
    reviewAttemptId: "plan-1:1",
    round: 1,
    rounds: 1,
    status: "done",
    summary: "Fix the missing null guard before completing the plan.",
    type: "review",
    verdict: "partial",
    ...overrides,
  };
}

describe("ReviewRow", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders running round + elapsed metadata while code review is in progress", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-28T03:01:05Z"));
    render(
      <ReviewRow
        item={buildReview({
          findings: undefined,
          round: 2,
          startedAt: Date.parse("2026-07-28T03:00:00Z"),
          status: "running",
          summary: null,
        })}
      />,
    );

    expect(screen.getByTestId("review-row-running-text").textContent).toBe("Reviewing code...");
    expect(screen.getByTestId("review-row-running-text").className).toContain("tc-loading-shimmer");
    expect(screen.getByTestId("review-row-running-meta").textContent).toBe(
      "Round 2 · 01:05 elapsed",
    );
    expect(screen.queryByTestId("review-row-toggle")).toBeNull();

    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByTestId("review-row-running-meta").textContent).toBe(
      "Round 2 · 01:06 elapsed",
    );
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

  it("renders a skipped review as one low-key non-expandable line", () => {
    render(
      <ReviewRow
        item={buildReview({
          findings: [],
          summary: "no code changes",
          verdict: "skipped",
        })}
      />,
    );

    expect(screen.getByTestId("review-row-skipped").textContent).toBe(
      "Code review skipped · no code changes",
    );
    expect(screen.queryByTestId("review-row-toggle")).toBeNull();
    expect(screen.queryByTestId("review-row-verdict")).toBeNull();
    expect(screen.queryByTestId("review-row-findings-count")).toBeNull();
  });
});
