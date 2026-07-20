import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProgressRow } from "../components/ProgressRow";

describe("ProgressRow", () => {
  it("renders a minimal dots placeholder while busy with no richer activity", () => {
    render(
      <ProgressRow
        busy
        hasActiveThinking={false}
        hasRunningTool={false}
        hasStreamingText={false}
        hasTodos={false}
      />,
    );

    expect(screen.getByTestId("progress-row-label").textContent).toBe("Thinking");
    expect(screen.getByTestId("progress-row-label").className).toContain("tc-loading-shimmer");
    expect(screen.getByTestId("progress-row-dots").textContent).toBe("...");
  });

  it("stays hidden when a more specific signal exists", () => {
    const { rerender } = render(
      <ProgressRow
        busy
        hasActiveThinking
        hasRunningTool={false}
        hasStreamingText={false}
        hasTodos={false}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();

    rerender(
      <ProgressRow
        busy
        hasActiveThinking={false}
        hasRunningTool
        hasStreamingText={false}
        hasTodos={false}
      />,
    );
    expect(screen.queryByTestId("progress-row")).toBeNull();
  });

  it("does not render when not busy", () => {
    render(
      <ProgressRow
        busy={false}
        hasActiveThinking={false}
        hasRunningTool={false}
        hasStreamingText={false}
        hasTodos={false}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();
  });
});
