import { fireEvent, render, screen } from "@testing-library/react";
import { createRef, type ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";

import { PlanFindWidget } from "./PlanFindWidget";

function renderWidget(overrides: Partial<ComponentProps<typeof PlanFindWidget>> = {}) {
  const props = {
    activeIndex: 0,
    inputRef: createRef<HTMLInputElement>(),
    onClose: vi.fn(),
    onNext: vi.fn(),
    onPrevious: vi.fn(),
    onQueryChange: vi.fn(),
    query: "plan",
    total: 17,
    ...overrides,
  };
  render(<PlanFindWidget {...props} />);
  return props;
}

describe("PlanFindWidget", () => {
  it("shows the current result and total with Cursor-style wording", () => {
    renderWidget({ activeIndex: 3, total: 17 });

    expect(screen.getByTestId("plan-find-count").textContent).toBe("4 of 17");
  });

  it("shows no-results feedback and disables navigation when a query has no match", () => {
    renderWidget({ total: 0 });

    expect(screen.getByTestId("plan-find-count").textContent).toBe("No results");
    expect(screen.getByTestId("plan-find-count").className).toContain("--empty");
    expect(screen.getByTestId("plan-find-prev").hasAttribute("disabled")).toBe(true);
    expect(screen.getByTestId("plan-find-next").hasAttribute("disabled")).toBe(true);
  });

  it("hides the count until the user enters a query", () => {
    renderWidget({ query: "", total: 0 });

    expect(screen.queryByTestId("plan-find-count")).toBeNull();
  });
  it("styles and marks the input invalid when no results are found", () => {
    renderWidget({ total: 0 });

    expect(screen.getByTestId("plan-find-input").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByTestId("plan-find-input").className).toContain("--empty");
  });

  it("forwards changes, navigation, keyboard navigation and close requests", () => {
    const props = renderWidget();
    const input = screen.getByTestId("plan-find-input");

    fireEvent.change(input, { target: { value: "todo" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    fireEvent.click(screen.getByTestId("plan-find-prev"));
    fireEvent.click(screen.getByTestId("plan-find-next"));
    fireEvent.click(screen.getByTestId("plan-find-close"));

    expect(props.onQueryChange).toHaveBeenCalledWith("todo");
    expect(props.onNext).toHaveBeenCalledTimes(2);
    expect(props.onPrevious).toHaveBeenCalledTimes(2);
    expect(props.onClose).toHaveBeenCalledOnce();
  });

  it("focuses and selects the search input when it opens", () => {
    const inputRef = createRef<HTMLInputElement>();
    const focus = vi.spyOn(HTMLInputElement.prototype, "focus");
    const select = vi.spyOn(HTMLInputElement.prototype, "select");

    renderWidget({ inputRef });

    expect(focus).toHaveBeenCalled();
    expect(select).toHaveBeenCalled();
  });
});
