import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { RestoreConfirmDialog } from "./RestoreConfirmDialog";

describe("RestoreConfirmDialog", () => {
  it("renders multi-file copy and the three action buttons", () => {
    render(
      <RestoreConfirmDialog
        changedFiles={["src/app.ts", "src/state.ts"]}
        onCancel={vi.fn()}
        onDontRevert={vi.fn()}
        onRevert={vi.fn()}
      />,
    );

    expect(screen.getByTestId("cp-confirm-dialog")).toBeTruthy();
    expect(screen.getByText("Restore to this checkpoint?")).toBeTruthy();
    expect(screen.getByTestId("cp-confirm-body").textContent).toContain("2 changed files");
    expect(screen.getByTestId("cp-confirm-cancel").textContent).toContain("Esc");
    expect(screen.getByTestId("cp-confirm-dont-revert").textContent).toContain("⇧↵");
    expect(screen.getByTestId("cp-confirm-revert").textContent).toContain("↵");
    expect(document.activeElement).toBe(screen.getByTestId("cp-confirm-revert"));
  });

  it("maps keyboard shortcuts to cancel, don't revert, and revert", () => {
    const onCancel = vi.fn();
    const onDontRevert = vi.fn();
    const onRevert = vi.fn();

    const { unmount } = render(
      <RestoreConfirmDialog
        changedFiles={["src/app.ts"]}
        onCancel={onCancel}
        onDontRevert={onDontRevert}
        onRevert={onRevert}
      />,
    );

    fireEvent.keyDown(document, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(document, { key: "Enter", shiftKey: true });
    expect(onDontRevert).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(document, { key: "Enter" });
    expect(onRevert).toHaveBeenCalledTimes(1);

    unmount();
  });
});
