import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProgressRow } from "../components/ProgressRow";

describe("ProgressRow", () => {
  it("shows title with N/M and spinner when progress exists", () => {
    render(
      <ProgressRow
        busy
        planState="executing"
        planTodos={[
          { content: "Run tests", id: "1", status: "completed" },
          { content: "Verify", id: "2", status: "in_progress" },
        ]}
        sessionTodos={[]}
      />,
    );

    expect(screen.getByTestId("progress-row-text").textContent).toBe("Verify (2/2)");
  });

  it("falls back to Working… when busy without todo data", () => {
    render(
      <ProgressRow
        busy
        planState="chat"
        planTodos={[]}
        sessionTodos={[]}
      />,
    );

    expect(screen.getByTestId("progress-row").textContent).toContain("Working…");
  });

  it("does not render when not busy", () => {
    const { container } = render(
      <ProgressRow
        busy={false}
        planState="executing"
        planTodos={[{ content: "A", id: "1", status: "in_progress" }]}
        sessionTodos={[]}
      />,
    );

    expect(container.innerHTML).toBe("");
  });
});
