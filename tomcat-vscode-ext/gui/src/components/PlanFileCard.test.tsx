import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PlanFileCard } from "./PlanFileCard";

describe("PlanFileCard", () => {
  it("renders planTodos with status classes", () => {
    render(
      <PlanFileCard
        item={{
          id: "plan-1",
          path: "/tmp/demo.plan.md",
          planId: "p1",
          state: "executing",
          type: "plan",
        }}
        onOpenPlanFile={() => undefined}
        planTodos={[
          { content: "Step 1", id: "1", status: "completed" },
          { content: "Step 2", id: "2", status: "in_progress" },
          { content: "Step 3", id: "3", status: "pending" },
          { content: "Step 4", id: "4", status: "cancelled" },
        ]}
      />,
    );

    expect(screen.getByTestId("plan-todos")).toBeTruthy();
    expect(screen.getByTestId("plan-todo-completed")).toBeTruthy();
    expect(screen.getByTestId("plan-todo-in_progress")).toBeTruthy();
    expect(screen.getByTestId("plan-todo-pending")).toBeTruthy();
    expect(screen.getByTestId("plan-todo-cancelled")).toBeTruthy();
  });
});
