import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlanActionStrip } from "./PlanActionStrip";

function renderStrip(overrides: Partial<Parameters<typeof PlanActionStrip>[0]> = {}) {
  const props = {
    availableModels: ["gpt-5.6", "claude-opus"],
    buildModel: "",
    canBuild: true,
    onBuild: vi.fn(),
    onSetBuildModel: vi.fn(),
    ...overrides,
  };
  render(<PlanActionStrip {...props} />);
  return props;
}

describe("PlanActionStrip", () => {
  it("shows a yellow Build button and the model dropdown, but no path or mode toggle", () => {
    renderStrip();
    const strip = screen.getByTestId("plan-action-strip");
    expect(strip.classList.contains("tc-plan-action-strip")).toBe(true);
    expect(screen.getByTestId("plan-build").classList.contains("tc-plan-build-button")).toBe(true);
    expect(screen.getByTestId("plan-build-model-select")).toBeTruthy();
    expect(screen.queryByTestId("plan-path")).toBeNull();
    expect(screen.queryByTestId("plan-overflow-trigger")).toBeNull();
  });

  it("emits onBuild when the Build button is clicked", () => {
    const props = renderStrip();
    fireEvent.click(screen.getByTestId("plan-build"));
    expect(props.onBuild).toHaveBeenCalledTimes(1);
  });

  it("disables Build when canBuild is false", () => {
    renderStrip({ canBuild: false });
    expect((screen.getByTestId("plan-build") as HTMLButtonElement).disabled).toBe(true);
  });

  it("routes model selection to onSetBuildModel", () => {
    const props = renderStrip();
    fireEvent.change(screen.getByTestId("plan-build-model-select"), {
      target: { value: "claude-opus" },
    });
    expect(props.onSetBuildModel).toHaveBeenCalledWith("claude-opus");
  });

  it("shows no visible 'Build model' label text (Cursor-flat)", () => {
    renderStrip();
    expect(screen.queryByText("Build model")).toBeNull();
    expect(document.querySelector(".tc-plan-model-select__label")).toBeNull();
  });
});
