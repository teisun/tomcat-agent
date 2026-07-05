import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ThinkingBlock } from "./ThinkingBlock";

describe("ThinkingBlock", () => {
  it("keeps the summary visible after streaming completes", () => {
    const item = {
      id: "thinking-1",
      text: "Inspect transcript grouping before touching provider state.",
      type: "thinking" as const,
    };

    const { rerender } = render(<ThinkingBlock isStreaming item={item} />);
    expect(screen.getByTestId("thinking-summary").textContent).toContain(
      "Inspect transcript grouping before touching provider state.",
    );
    expect(screen.getByTestId("thinking-status").className).toContain("codicon-loading");

    rerender(<ThinkingBlock item={item} />);

    expect(screen.getByTestId("thinking-summary").textContent).toContain(
      "Inspect transcript grouping before touching provider state.",
    );
    expect(screen.getByTestId("thinking-status").className).toContain("codicon-check");
  });

  it("expands the body when toggled", () => {
    render(
      <ThinkingBlock
        item={{
          id: "thinking-2",
          text: "Need to verify the historical assistantMessageId mapping.",
          type: "thinking",
        }}
      />,
    );

    expect(screen.queryByTestId("thinking-body")).toBeNull();
    fireEvent.click(screen.getByTestId("thinking-toggle"));
    expect(screen.getByTestId("thinking-body").textContent).toContain(
      "historical assistantMessageId mapping",
    );
  });
});
