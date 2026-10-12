import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProgressRow } from "../components/ProgressRow";

describe("ProgressRow", () => {
  it("renders the dots row whenever the turn is still busy", () => {
    render(<ProgressRow busy />);

    expect(screen.getByRole("status", { name: "Still working" })).toBeTruthy();
    expect(screen.queryByTestId("progress-row-label")).toBeNull();
    expect(screen.queryByText("Thinking")).toBeNull();
    expect(screen.getByTestId("progress-row-dots").querySelectorAll(".tc-loading-dots__dot")).toHaveLength(
      3,
    );
  });

  it("keeps the same plain dots row for the whole busy turn", () => {
    const { rerender } = render(<ProgressRow busy />);

    expect(screen.getByTestId("progress-row").className).toBe("tc-progress-row");

    rerender(<ProgressRow busy />);

    expect(screen.getByTestId("progress-row").className).toBe("tc-progress-row");
    expect(screen.getByTestId("progress-row-dots").querySelectorAll(".tc-loading-dots__dot")).toHaveLength(
      3,
    );
  });

  it("does not render when not busy", () => {
    render(<ProgressRow busy={false} />);

    expect(screen.queryByTestId("progress-row")).toBeNull();
  });
});
