import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { KeySlotCombobox, type KeySlotOption } from "./KeySlotCombobox";

const DEFAULT_OPTIONS: KeySlotOption[] = [
  {
    envName: "ALPHA_KEY",
    group: "suggested",
    keyPresent: false,
    label: "Alpha provider",
  },
  {
    envName: "BETA_KEY",
    group: "saved",
    keyPresent: true,
    label: "Beta provider",
  },
];

function renderCombobox(
  overrides: Partial<{
    options: KeySlotOption[];
    refreshDisabled: boolean;
    value: string;
  }> = {},
) {
  const onChange = vi.fn();
  const onRefresh = vi.fn();

  function Harness() {
    const [value, setValue] = useState(overrides.value ?? "");
    return (
      <KeySlotCombobox
        feedback={null}
        hint="Choose or create a key slot."
        onChange={(nextEnvName) => {
          onChange(nextEnvName);
          setValue(nextEnvName);
        }}
        onRefresh={onRefresh}
        options={overrides.options ?? DEFAULT_OPTIONS}
        placeholder="Select a key slot"
        refreshDisabled={overrides.refreshDisabled ?? false}
        refreshLabel="Refresh key slots"
        refreshing={false}
        value={value}
      />
    );
  }

  const utils = render(<Harness />);
  return {
    ...utils,
    input: screen.getByRole("combobox", { name: "Key slot" }) as HTMLInputElement,
    onChange,
    onRefresh,
  };
}

describe("KeySlotCombobox", () => {
  it("supports keyboard navigation and exposes combobox accessibility attributes", () => {
    const { input, onChange } = renderCombobox();

    fireEvent.focus(input);
    expect(input.getAttribute("aria-expanded")).toBe("true");
    expect(input.getAttribute("aria-controls")).toBeTruthy();
    expect(screen.getByRole("listbox").id).toBe(input.getAttribute("aria-controls"));

    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onChange).toHaveBeenLastCalledWith("BETA_KEY");
    expect(input.value).toBe("BETA_KEY");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("closes on outside click and removes the mousedown listener on unmount", () => {
    const addSpy = vi.spyOn(window, "addEventListener");
    const removeSpy = vi.spyOn(window, "removeEventListener");
    const { input, unmount } = renderCombobox();

    fireEvent.focus(input);
    expect(screen.getByRole("listbox")).toBeTruthy();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.focus(input);
    const pointerHandler = addSpy.mock.calls.find((call) => call[0] === "mousedown")?.[1];
    expect(pointerHandler).toBeTruthy();
    unmount();
    expect(removeSpy).toHaveBeenCalledWith("mousedown", pointerHandler);

    addSpy.mockRestore();
    removeSpy.mockRestore();
  });

  it("keeps invalid custom entries disabled and ignores Enter for them", () => {
    const { input, onChange } = renderCombobox({ options: [] });

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "bad-key-slot" } });

    const invalidCreate = screen.getByRole("option", { name: /bad-key-slot/i });
    expect(invalidCreate.className).toContain("tc-settings-combobox__item--disabled");
    expect(
      screen.getByText("Use uppercase letters, numbers, and underscores"),
    ).toBeTruthy();

    const callCountBeforeEnter = onChange.mock.calls.length;
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).toHaveBeenCalledTimes(callCountBeforeEnter);
    expect(screen.getByRole("listbox")).toBeTruthy();
    expect(input.value).toBe("bad-key-slot");
  });

  it("shows the empty state and allows committing a valid custom slot", () => {
    const { input, onChange } = renderCombobox({ options: [] });

    fireEvent.focus(input);
    expect(screen.getByText("No matching key slots.")).toBeTruthy();

    fireEvent.change(input, { target: { value: "NEW_SLOT" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onChange).toHaveBeenLastCalledWith("NEW_SLOT");
    expect(input.value).toBe("NEW_SLOT");
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
