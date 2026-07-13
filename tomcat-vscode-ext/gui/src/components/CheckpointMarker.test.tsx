import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CheckpointMarker } from "./CheckpointMarker";

describe("CheckpointMarker", () => {
  it("opens restore flow when clicked", () => {
    const onRestore = vi.fn();

    render(
      <CheckpointMarker
        item={{
          changedFiles: ["src/app.ts"],
          checkpointId: "ck-1",
          createdAt: "2026-07-12T12:00:00Z",
          id: "marker-1",
          kind: "turn_end",
          messageAnchor: "assistant-1",
          type: "checkpoint",
        }}
        onRestore={onRestore}
      />,
    );

    fireEvent.click(screen.getByTestId("checkpoint-marker-button"));
    expect(onRestore).toHaveBeenCalledWith(
      expect.objectContaining({
        checkpointId: "ck-1",
        changedFiles: ["src/app.ts"],
      }),
    );
  });
});
