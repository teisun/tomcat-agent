import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewTimelineItem } from "../types";
import { TranscriptView } from "./TranscriptView";

describe("TranscriptView", () => {
  it("keeps a high-signal edit tool visible as a standalone row", () => {
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Created");
    expect(screen.queryByTestId("thinking-group")).toBeNull();
  });

  it("folds context tools into a thinking group while keeping action tools visible", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-1",
        id: "assistant-msg-1",
        kind: "assistant",
        text: "I'll inspect and then update the file.",
        type: "message",
      },
      {
        assistantMessageId: "assistant-1",
        id: "think-1",
        summaryTitle: "Searched 1 source",
        text: "Look at the original file first.",
        type: "thinking",
      },
      {
        args: { path: "/workspace/a.rs" },
        assistantMessageId: "assistant-1",
        id: "tool-read",
        isError: false,
        status: "complete",
        summary: "fn main() {}",
        toolCallId: "tc-read",
        toolName: "read",
        type: "tool",
      },
      {
        args: { path: "/workspace/a.rs" },
        assistantMessageId: "assistant-1",
        diffStat: { added: 1, removed: 1 },
        display: { file: "/workspace/a.rs", kind: "file" },
        id: "tool-edit",
        isError: false,
        status: "complete",
        summary: "updated file",
        toolCallId: "tc-edit",
        toolName: "edit",
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

    expect(screen.getByTestId("thinking-group")).toBeTruthy();
    expect(screen.getByTestId("thinking-group-title").textContent).toContain("Searched 1 source");
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Edited");
    expect(screen.queryAllByTestId("tool-row")).toHaveLength(1);
  });

  it("renders a single context tool without thinking as a standalone file-chip row", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        args: { path: "/workspace/README.md" },
        assistantMessageId: "assistant-1",
        display: { file: "/workspace/README.md", kind: "file" },
        id: "tool-read",
        isError: false,
        status: "complete",
        summary: "# readme",
        toolCallId: "tc-read",
        toolName: "read",
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

    expect(screen.queryByTestId("thinking-group")).toBeNull();
    expect(screen.getByTestId("file-chip").textContent).toContain("README.md");
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Read");
  });
});
