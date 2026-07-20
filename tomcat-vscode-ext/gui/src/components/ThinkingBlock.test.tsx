import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ThinkingBlock } from "./ThinkingBlock";

describe("ThinkingBlock", () => {
  it("labels the standalone block as Thinking without the product prefix", () => {
    render(
      <ThinkingBlock
        item={{
          id: "thinking-label",
          text: "Trace the bridge event before the first streamed token arrives.",
          type: "thinking",
        }}
      />,
    );

    expect(screen.getByText("Thinking")).toBeTruthy();
    expect(screen.queryByText("Tomcat · Thinking")).toBeNull();
  });

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
    expect(screen.getByTestId("thinking-status").className).toContain("codicon-lightbulb");
    expect(screen.getByTestId("thinking-status").className).not.toContain("codicon-loading");
    expect(screen.getByTestId("thinking-status").className).not.toContain("tc-codicon-spin");
    expect(screen.queryByTestId("thinking-streaming-indicator")).toBeNull();

    rerender(<ThinkingBlock item={item} />);

    expect(screen.getByTestId("thinking-summary").textContent).toContain(
      "Inspect transcript grouping before touching provider state.",
    );
    expect(screen.getByTestId("thinking-status").className).toContain("codicon-lightbulb");
    expect(screen.getByTestId("thinking-status").className).not.toContain("codicon-check");
    expect(screen.queryByTestId("thinking-streaming-indicator")).toBeNull();
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

  it("renders expanded thinking as plain preformatted text", () => {
    render(
      <ThinkingBlock
        item={{
          id: "thinking-3",
          text: "## Inspect\n\nStart with `src/ui/provider.ts:12`.",
          type: "thinking",
        }}
      />,
    );

    fireEvent.click(screen.getByTestId("thinking-toggle"));
    const body = screen.getByTestId("thinking-body");
    expect(body.tagName).toBe("PRE");
    expect(body.textContent).toContain("## Inspect");
    expect(body.textContent).toContain("`src/ui/provider.ts:12`");
    expect(body.querySelector("h2")).toBeNull();
    expect(body.querySelector("strong")).toBeNull();
  });
});
