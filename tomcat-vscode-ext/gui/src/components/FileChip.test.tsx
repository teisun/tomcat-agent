import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { FileChip, fileChipIconClass } from "./FileChip";

describe("FileChip", () => {
  it("renders label and maps extension to codicon", () => {
    render(<FileChip onOpenFile={vi.fn()} path="/workspace/src/main.rs" />);

    expect(screen.getByTestId("file-chip-label").textContent).toBe("main.rs");
    expect(screen.getByTestId("file-chip-icon").className).toContain("codicon-file-code");
    expect(fileChipIconClass("docs/readme.md")).toContain("codicon-book");
  });

  it("click triggers openFile with full path", () => {
    const onOpenFile = vi.fn();
    render(<FileChip onOpenFile={onOpenFile} path="/workspace/Cargo.toml" />);

    fireEvent.click(screen.getByTestId("file-chip"));
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/Cargo.toml");
  });
});
