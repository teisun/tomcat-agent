import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlanBuildModelSelect } from "./PlanBuildModelSelect";

describe("PlanBuildModelSelect", () => {
  it("lists the available models plus a session-default option and reflects the current value", () => {
    render(
      <PlanBuildModelSelect
        availableModels={["gpt-5.6", "claude-opus"]}
        onChange={() => undefined}
        value="claude-opus"
      />,
    );
    const select = screen.getByTestId("plan-build-model-select") as HTMLSelectElement;
    expect(select.value).toBe("claude-opus");
    expect(Array.from(select.options).map((option) => option.value)).toEqual([
      "",
      "gpt-5.6",
      "claude-opus",
    ]);
  });

  it("invokes onChange with the selected model id", () => {
    const onChange = vi.fn();
    render(
      <PlanBuildModelSelect
        availableModels={["gpt-5.6", "claude-opus"]}
        onChange={onChange}
        value=""
      />,
    );
    fireEvent.change(screen.getByTestId("plan-build-model-select"), {
      target: { value: "gpt-5.6" },
    });
    expect(onChange).toHaveBeenCalledWith("gpt-5.6");
  });

  it("falls back to the empty option when the value is no longer available", () => {
    render(
      <PlanBuildModelSelect availableModels={["gpt-5.6"]} onChange={() => undefined} value="stale" />,
    );
    expect((screen.getByTestId("plan-build-model-select") as HTMLSelectElement).value).toBe("");
  });

  it("disables the dropdown when there are no ready models", () => {
    render(<PlanBuildModelSelect availableModels={[]} onChange={() => undefined} value="" />);
    expect((screen.getByTestId("plan-build-model-select") as HTMLSelectElement).disabled).toBe(true);
  });

  it("renders only the select (no visible text label) but keeps an accessible name", () => {
    render(
      <PlanBuildModelSelect
        availableModels={["gpt-5.6"]}
        label="Build model"
        onChange={() => undefined}
        value=""
      />,
    );
    expect(screen.getByLabelText("Build model")).toBeTruthy();
    expect(document.querySelector(".tc-plan-model-select__label")).toBeNull();
    expect(screen.queryByText("Build model")).toBeNull();
  });
});
