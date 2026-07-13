import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { DisclosureCard } from "./DisclosureCard";

describe("DisclosureCard", () => {
  it("shows preview while collapsed and expands into the full body", () => {
    render(
      <DisclosureCard
        header={<span>Header</span>}
        leadingIcon={<span data-testid="card-icon">icon</span>}
        preview={<div data-testid="preview-content">tail output</div>}
        statusVariant="success"
        toggleTestId="toggle"
      >
        <div data-testid="body-content">full output</div>
      </DisclosureCard>,
    );

    expect(
      screen.getByTestId("disclosure-card-leading-icon").contains(screen.getByTestId("card-icon")),
    ).toBe(true);
    expect(screen.getByTestId("preview-content").textContent).toContain("tail output");
    expect(screen.queryByTestId("body-content")).toBeNull();

    fireEvent.click(screen.getByTestId("toggle"));

    expect(screen.getByTestId("body-content").textContent).toContain("full output");
    expect(screen.queryByTestId("preview-content")).toBeNull();
  });
});
