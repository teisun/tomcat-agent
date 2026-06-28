import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewTimelineItem } from "../types";
import { TranscriptView } from "./TranscriptView";

describe("TranscriptView", () => {
  it("promotes a single tool without thinking into a standalone ToolRow", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-1",
        id: "assistant-msg-1",
        kind: "assistant",
        text: "I can update the file now.",
        type: "message",
      },
      {
        args: { path: "/workspace/sega-run-gun.html" },
        assistantMessageId: "assistant-1",
        display: { file: "/workspace/sega-run-gun.html", kind: "file" },
        id: "tool-1",
        isError: false,
        status: "complete",
        summary: "updated file",
        toolCallId: "tc-1",
        toolName: "write",
        type: "tool",
      },
    ];

    render(
      <TranscriptView
        busy={false}
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={timeline}
      />,
    );

    expect(screen.getByText("I can update the file now.")).toBeTruthy();
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Edited");
    expect(screen.queryByTestId("thinking-group")).toBeNull();
  });
});
