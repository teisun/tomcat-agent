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
    expect(screen.getByTestId("build-plan").classList.contains("tc-plan-build-button")).toBe(true);
    expect(screen.getByTestId("build-plan").classList.contains("tc-button--primary")).toBe(false);

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

  it("prefers the card's own todos count over the session planTodos prop", () => {
    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-4",
          path: "/tmp/own.plan.md",
          planId: "p4",
          state: "planning",
          todos: [
            { content: "Own 1", id: "o1", status: "pending" },
            { content: "Own 2", id: "o2", status: "in_progress" },
          ],
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[
          { content: "Session 1", id: "s1", status: "pending" },
          { content: "Session 2", id: "s2", status: "pending" },
          { content: "Session 3", id: "s3", status: "pending" },
        ]}
      />,
    );

    expect(screen.getByTestId("plan-todos-count").textContent).toBe("2 todos");
  });

  it("falls back to session planTodos when the card has no todos of its own", () => {
    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-5",
          path: "/tmp/fallback.plan.md",
          planId: "p5",
          state: "planning",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[
          { content: "Session 1", id: "s1", status: "pending" },
        ]}
      />,
    );

    expect(screen.getByTestId("plan-todos-count").textContent).toBe("1 todo");
  });

  it("renders the build-model dropdown between View Plan and Build when onSetBuildModel is provided", () => {
    const onSetBuildModel = vi.fn();
    const onBuild = vi.fn();

    render(
      <PlanFileCard
        availableModels={["gpt-5.6", "claude-opus"]}
        buildModel="gpt-5.6"
        canBuild
        item={{
          id: "plan-model",
          path: "/tmp/model.plan.md",
          planId: "pm",
          state: "planning",
          type: "plan",
        }}
        onBuild={onBuild}
        onOpenPlanFile={() => undefined}
        onSetBuildModel={onSetBuildModel}
        planTodos={[]}
      />,
    );

    const select = screen.getByTestId("plan-card-build-model") as HTMLSelectElement;
    expect(select.value).toBe("gpt-5.6");
    fireEvent.change(select, { target: { value: "claude-opus" } });
    expect(onSetBuildModel).toHaveBeenCalledWith("claude-opus");

    fireEvent.click(screen.getByTestId("build-plan"));
    expect(onBuild).toHaveBeenCalledWith("pm", "/tmp/model.plan.md");

    // Flat dropdown: no visible "Model" text label, but an accessible name stays.
    expect(document.querySelector(".tc-plan-model-select__label")).toBeNull();
    expect(screen.queryByText("Model")).toBeNull();
    expect(screen.getByLabelText("Model")).toBeTruthy();
  });

  it("omits the build-model dropdown when onSetBuildModel is not provided", () => {
    render(
      <PlanFileCard
        canBuild
        item={{
          id: "plan-no-model",
          path: "/tmp/no-model.plan.md",
          planId: "pnm",
          state: "planning",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[]}
      />,
    );
    expect(screen.queryByTestId("plan-card-build-model")).toBeNull();
  });

  it("restores the legacy pending footer while a plan card is still creating", () => {
    render(
      <PlanFileCard
        canBuild
        creating
        item={{
          id: "plan-creating",
          path: "/tmp/creating.plan.md",
          planId: "pc",
          state: "planning",
          type: "plan",
        }}
        onBuild={() => undefined}
        onOpenPlanFile={() => undefined}
        planTodos={[]}
      />,
    );

    expect((screen.getByTestId("view-plan-pending") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByTestId("view-plan")).toBeNull();
    expect(screen.getByTestId("build-plan")).toBeTruthy();
  });
});
