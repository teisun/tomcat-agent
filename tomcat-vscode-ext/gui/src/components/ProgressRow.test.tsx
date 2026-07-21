import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProgressRow } from "../components/ProgressRow";

describe("ProgressRow", () => {
  it("renders only breathing dots while busy with no richer activity", () => {
    render(
      <ProgressRow
        busy
        hasActiveThinking={false}
        hasRunningTool={false}
        hasStreamingText={false}
      />,
    );

    expect(screen.getByRole("status", { name: "Waiting for more output" })).toBeTruthy();
    expect(screen.queryByTestId("progress-row-label")).toBeNull();
    expect(screen.queryByText("Thinking")).toBeNull();
    expect(screen.getByTestId("progress-row-dots").querySelectorAll(".tc-loading-dots__dot")).toHaveLength(
      3,
    );
  });

  it("stays hidden when a more specific signal exists", () => {
    const { rerender } = render(
      <ProgressRow
        busy
        hasActiveThinking
        hasRunningTool={false}
        hasStreamingText={false}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();

    rerender(
      <ProgressRow
        busy
        hasActiveThinking={false}
        hasRunningTool
        hasStreamingText={false}
      />,
    );
    expect(screen.queryByTestId("progress-row")).toBeNull();

    rerender(
      <ProgressRow
        busy
        hasActiveThinking={false}
        hasRunningTool={false}
        hasStreamingText
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
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();
  });
});
