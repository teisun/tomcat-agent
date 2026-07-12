import { render, screen, within } from "@testing-library/react";
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

  it("keeps a grouped create_plan turn to a single header while showing a pending plan card affordance", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-plan",
        id: "think-plan",
        summaryTitle: "Creating plan",
        text: "Let me break the work down first.",
        type: "thinking",
      },
      {
        assistantMessageId: "assistant-plan",
        display: { kind: "plan", plan: "/tmp/demo.plan.md" },
        id: "tool-plan",
        isError: false,
        status: "streaming",
        summary: "Created plan demo",
        toolCallId: "tc-plan",
        toolName: "create_plan",
        type: "tool",
      },
      {
        id: "plan-card-1",
        path: "/tmp/demo.plan.md",
        planId: "demo-plan",
        state: "planning",
        title: "Demo plan",
        type: "plan",
      },
    ];

    render(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={timeline}
      />,
    );

    expect(screen.getAllByText("Creating plan")).toHaveLength(1);
    expect(screen.getByTestId("thinking-group")).toBeTruthy();
    expect(screen.getByTestId("plan-card-title").textContent).toBe("Demo plan");
    expect(screen.getByTestId("view-plan-pending")).toBeTruthy();
  });

  it("keeps a standalone create_plan tool visible when no thinking text exists", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-plan-standalone",
        id: "tool-plan-only",
        isError: false,
        status: "complete",
        summary: "Created plan mini-game",
        toolCallId: "tc-plan-only",
        toolName: "create_plan",
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
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Created plan");
  });

  it("prefers matching by planId before falling back to the newest plan card", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "plan-card-a",
        path: "/tmp/plan-a.plan.md",
        planId: "plan-a",
        state: "planning",
        title: "Plan A",
        type: "plan",
      },
      {
        id: "plan-card-b",
        path: "/tmp/plan-b.plan.md",
        planId: "plan-b",
        state: "planning",
        title: "Plan B",
        type: "plan",
      },
      {
        args: { plan_id: "plan-a" },
        assistantMessageId: "assistant-update",
        display: { kind: "plan", plan: "/tmp/plan-a.plan.md" },
        id: "tool-update-plan",
        isError: false,
        status: "streaming",
        summary: "Updating plan",
        toolCallId: "tc-update-plan",
        toolName: "update_plan",
        type: "tool",
      },
    ];

    render(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={timeline}
      />,
    );

    const planCards = screen.getAllByTestId("plan-card");
    expect(within(planCards[0]).getByTestId("plan-card-title").textContent).toBe("Plan A");
    expect(within(planCards[0]).getByTestId("view-plan-pending")).toBeTruthy();
    expect(within(planCards[1]).getByTestId("plan-card-title").textContent).toBe("Plan B");
    expect(within(planCards[1]).getByTestId("view-plan")).toBeTruthy();
  });
});
