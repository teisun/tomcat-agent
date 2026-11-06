import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProgressRow } from "../components/ProgressRow";
import { translate } from "../../../src/shared/i18n";

describe("ProgressRow", () => {
  it("updates one live retry row and returns to ordinary progress when the request settles", () => {
    const { rerender } = render(<ProgressRow busy />);
    for (const requestRetry of [
      { attempt: 2, maxAttempts: 10 }, { attempt: 3, maxAttempts: 10 },
      { attempt: 10, maxAttempts: 10 }, { attempt: 2, maxAttempts: 4 },
    ]) {
      rerender(<ProgressRow busy requestRetry={requestRetry} />);
      const label = translate("en", "progress.retrying", requestRetry);
      expect(screen.getAllByTestId("progress-row")).toHaveLength(1);
      expect(screen.getByRole("status", { name: label })).toBeTruthy();
      expect(screen.getByTestId("progress-row-label").textContent).toBe(label);
    }
    rerender(<ProgressRow busy />);
    expect(screen.queryByTestId("progress-row-label")).toBeNull();
    expect(screen.getByRole("status", { name: translate("en", "progress.working") })).toBeTruthy();
    rerender(<ProgressRow busy={false} requestRetry={{ attempt: 10, maxAttempts: 10 }} />);
    expect(screen.queryByTestId("progress-row")).toBeNull();
  });

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
