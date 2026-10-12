import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlanActionStrip } from "./PlanActionStrip";

function renderStrip(overrides: Partial<Parameters<typeof PlanActionStrip>[0]> = {}) {
  const props = {
    availableModels: ["gpt-5.6", "claude-opus"],
    buildModel: "",
    canBuild: true,
    fileState: "planning" as const,
    onBuild: vi.fn(),
    onSelectContextWindow: vi.fn(),
    onSelectThinkingLevel: vi.fn(),
    onSelectSpeed: vi.fn(),
    onSetBuildModel: vi.fn(),
    sessionModel: "gpt-5.6",
    ...overrides,
  };
  render(<PlanActionStrip {...props} />);
  return props;
}

describe("PlanActionStrip", () => {
  it("passes Speed through the plan picker without switching its build model", () => {
    const onSelectSpeed = vi.fn();
    const props = renderStrip({
      availableModelDetails: { "gpt-5.6": { id: "gpt-5.6", supportedSpeeds: ["fast"], selectedSpeed: "standard" } },
      onSelectSpeed,
    });
    fireEvent.click(screen.getByTestId("plan-build-model-select"));
    const edit = screen.getByTestId("model-edit-gpt-5.6");
    fireEvent.mouseEnter(edit.closest(".tc-model-picker-option")!);
    fireEvent.click(edit);
    fireEvent.click(screen.getByRole("button", { name: "Fast" }));
    expect(onSelectSpeed).toHaveBeenCalledWith("gpt-5.6", "fast");
    expect(props.onSetBuildModel).not.toHaveBeenCalled();
  });

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

  it("labels a pending plan action as Resume", () => {
    renderStrip({ fileState: "pending" });
    expect(screen.getByTestId("plan-build").textContent).toBe("Resume");
  });

  it("disables Build when canBuild is false", () => {
    renderStrip({ canBuild: false });
    expect((screen.getByTestId("plan-build") as HTMLButtonElement).disabled).toBe(true);
  });

  it("routes model selection to onSetBuildModel", () => {
    const props = renderStrip();
    const picker = screen.getByTestId("plan-build-model-select");
    expect(picker.classList.contains("tc-model-picker-trigger")).toBe(true);
    fireEvent.click(picker);
    expect(document.querySelector(".tc-model-picker-dropdown--below")).toBeTruthy();
    expect(picker.querySelector(".codicon-chevron-down")).toBeTruthy();
    fireEvent.click(screen.getAllByTestId("model-option")[1]);
    expect(props.onSetBuildModel).toHaveBeenCalledWith("claude-opus");
  });

  it("shows no visible 'Build model' label text (Cursor-flat)", () => {
    renderStrip();
    expect(screen.queryByText("Build model")).toBeNull();
    expect(document.querySelector(".tc-plan-model-select__label")).toBeNull();
  });
});
