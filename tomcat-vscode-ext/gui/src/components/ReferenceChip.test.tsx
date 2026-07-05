import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ReferenceChip } from "./ReferenceChip";

describe("ReferenceChip", () => {
  it("shows the full file and line range in the tooltip", () => {
    render(
      <ReferenceChip
        reference={{
          kind: "selection",
          label: "app.ts:8-12",
          lineEnd: 12,
          lineStart: 8,
          path: "src/app.ts",
          text: "const total = items.length;",
          type: "reference",
        }}
      />,
    );

    expect(screen.getByTestId("reference-chip").getAttribute("title")).toBe("src/app.ts:8-12");
    expect(screen.getByTestId("reference-chip").getAttribute("aria-label")).toBe(
      "src/app.ts:8-12",
    );
  });

  it("invokes removal when the dismiss button is clicked", () => {
    const onRemove = vi.fn();
    render(
      <ReferenceChip
        onRemove={onRemove}
        reference={{
          kind: "file",
          label: "src/app.ts",
          path: "src/app.ts",
          type: "reference",
        }}
        testId="pending-reference-chip"
      />,
    );

    fireEvent.click(screen.getByTestId("pending-reference-chip-remove"));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});
