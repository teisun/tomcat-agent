import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlanFileCard } from "./PlanFileCard";

describe("PlanFileCard", () => {
  it("renders title, overview, todo count, and card actions", () => {
    const onBuild = vi.fn();
    const onOpenPlanFile = vi.fn();

    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-1",
          overview: "Review the transcript UI polish before building.",
          path: "/tmp/demo.plan.md",
          planId: "p1",
          state: "planning",
          title: "Demo Plan UI",
          type: "plan",
        }}
        onBuild={onBuild}
        onOpenPlanFile={onOpenPlanFile}
        planTodos={[
          { content: "Step 1", id: "1", status: "completed" },
          { content: "Step 2", id: "2", status: "in_progress" },
          { content: "Step 3", id: "3", status: "pending" },
          { content: "Step 4", id: "4", status: "cancelled" },
        ]}
      />,
    );

    expect(screen.getByTestId("plan-card-file-name").textContent).toBe("demo.plan.md");
    expect(screen.getByTestId("plan-card-title").textContent).toBe("Demo Plan UI");
    expect(screen.getByTestId("plan-card-overview").textContent).toContain("transcript UI polish");
    expect(screen.getByTestId("plan-todos-count").textContent).toBe("4 todos");

    fireEvent.click(screen.getByTestId("plan-card-title"));
    fireEvent.click(screen.getByTestId("view-plan"));
    fireEvent.click(screen.getByTestId("build-plan"));

    expect(onOpenPlanFile).toHaveBeenCalledTimes(2);
    expect(onOpenPlanFile).toHaveBeenCalledWith("/tmp/demo.plan.md");
    expect(onBuild).toHaveBeenCalledTimes(1);
  });

  it("disables Build when the active plan cannot build", () => {
    render(
      <PlanFileCard
        canBuild={false}
        item={{
          id: "plan-2",
          path: "/tmp/idle.plan.md",
          planId: "p2",
          state: "completed",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[]}
      />,
    );

    expect((screen.getByTestId("build-plan") as HTMLButtonElement).disabled).toBe(true);
  });
});
