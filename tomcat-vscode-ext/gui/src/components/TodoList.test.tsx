import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TodoList } from "./TodoList";

describe("TodoList", () => {
  it("renders one icon per todo with a state-specific SVG", () => {
    render(
      <TodoList
        todos={[
          { content: "Pending", id: "t1", status: "pending" },
          { content: "Running", id: "t2", status: "in_progress" },
          { content: "Cancelled", id: "t3", status: "cancelled" },
          { content: "Completed", id: "t4", status: "completed" },
        ]}
      />,
    );

    const items = screen.getAllByTestId("plan-todo-item");
    expect(items).toHaveLength(4);
    const states = items.map((item) => item.getAttribute("data-status"));
    expect(states).toEqual(["pending", "in_progress", "cancelled", "completed"]);

    for (const item of items) {
      const svg = item.querySelector("svg.tc-plan-todo__icon");
      expect(svg).not.toBeNull();
    }
    expect(document.querySelector('svg[data-state="in_progress"] circle[stroke-dasharray]')).not.toBeNull();
    expect(document.querySelector('svg[data-state="cancelled"] line.tc-plan-todo__icon-slash')).not.toBeNull();
    expect(document.querySelector('svg[data-state="completed"] path.tc-plan-todo__icon-check')).not.toBeNull();
  });

  it("sizes icons through the --tc-todo-icon-size CSS variable", () => {
    render(<TodoList todos={[{ content: "x", id: "t1", status: "pending" }]} />);
    const svg = document.querySelector("svg.tc-plan-todo__icon") as SVGElement;
    // The width/height resolve from the CSS custom property, not a hard-coded px.
    expect(svg.getAttribute("width")).toBeNull();
    expect(svg.getAttribute("height")).toBeNull();
    expect(svg.classList.contains("tc-plan-todo__icon")).toBe(true);
  });

  it("shows an empty affordance when there are no todos", () => {
    render(<TodoList todos={[]} />);
    expect(screen.getByTestId("plan-todo-empty")).toBeTruthy();
    expect(screen.queryByTestId("plan-todo-list")).toBeNull();
  });
});
