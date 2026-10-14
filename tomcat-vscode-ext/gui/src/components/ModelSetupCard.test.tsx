import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ModelSetupCard } from "./ModelSetupCard";
import { translate } from "../../../src/shared/i18n";

describe("ModelSetupCard", () => {
  it("shows one sentence and one Settings button, with no backend text", () => {
    const onConfigure = vi.fn();
    render(<ModelSetupCard onConfigure={onConfigure} />);
    expect(screen.getByRole("status").textContent).toBe(translate("en", "modelSetup.title") + translate("en", "modelSetup.configure"));
    fireEvent.click(screen.getByRole("button", { name: translate("en", "modelSetup.configure") }));
    expect(onConfigure).toHaveBeenCalledOnce();
  });
});
