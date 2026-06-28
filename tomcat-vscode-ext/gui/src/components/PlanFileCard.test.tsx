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

    fireEvent.click(screen.getByTestId("plan-card-file-link"));
    fireEvent.click(screen.getByTestId("plan-card-title"));
    fireEvent.click(screen.getByTestId("view-plan"));
    fireEvent.click(screen.getByTestId("build-plan"));

    expect(onOpenPlanFile).toHaveBeenCalledTimes(3);
    expect(onOpenPlanFile).toHaveBeenCalledWith("/tmp/demo.plan.md");
    expect(onBuild).toHaveBeenCalledTimes(1);
    expect(onBuild).toHaveBeenCalledWith("p1", "/tmp/demo.plan.md");
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

  it("keeps executing plans disabled even when the session can build", () => {
    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-2b",
          path: "/tmp/executing.plan.md",
          planId: "p2b",
          state: "executing",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[]}
      />,
    );

    expect((screen.getByTestId("build-plan") as HTMLButtonElement).disabled).toBe(true);
  });

  it("lets planning and pending cards build themselves when multiple cards are shown", () => {
    const onBuild = vi.fn();

    render(
      <>
        <PlanFileCard
          canBuild
          item={{
            id: "plan-a",
            path: "/tmp/plan-a.plan.md",
            planId: "plan-a",
            state: "planning",
            type: "plan",
          }}
          onBuild={onBuild}
          onOpenPlanFile={() => undefined}
          planTodos={[]}
        />
        <PlanFileCard
          canBuild
          item={{
            id: "plan-b",
            path: "/tmp/plan-b.plan.md",
            planId: "plan-b",
            state: "pending",
            type: "plan",
          }}
          onBuild={onBuild}
          onOpenPlanFile={() => undefined}
          planTodos={[]}
        />
      </>,
    );

    const buildButtons = screen.getAllByTestId("build-plan");
    expect((buildButtons[0] as HTMLButtonElement).disabled).toBe(false);
    expect((buildButtons[1] as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(buildButtons[0]);
    fireEvent.click(buildButtons[1]);

    expect(onBuild).toHaveBeenNthCalledWith(1, "plan-a", "/tmp/plan-a.plan.md");
    expect(onBuild).toHaveBeenNthCalledWith(2, "plan-b", "/tmp/plan-b.plan.md");
  });

  it("derives a cleaner semantic title when explicit title is missing", () => {
    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-3",
          overview: "Create a classic Sega-style run-and-gun game in a single HTML file.",
          path: "/tmp/plan_test_stuff__________md______html_c42aa6f6.plan.md",
          planId: "plan_test_stuff__________md______html_c42aa6f6",
          state: "planning",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[]}
      />,
    );

    expect(screen.getByTestId("plan-card-title").textContent).toBe(
      "Create a classic Sega-style run-and-gun game in a single HTML file.",
    );
  });
});
