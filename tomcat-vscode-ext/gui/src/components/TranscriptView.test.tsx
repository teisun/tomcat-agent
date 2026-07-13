import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewCheckpoint, WebviewTimelineItem } from "../types";
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

  it("renders checkpoint markers in sequence and forwards restore clicks", () => {
    const onRestoreCheckpoint = vi.fn();
    const timeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "first prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "first reply",
        type: "message",
      },
      {
        id: "user-2",
        kind: "user",
        text: "second prompt",
        type: "message",
      },
    ];
    const checkpoints: WebviewCheckpoint[] = [
      {
        changedFiles: ["src/app.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ];

    render(
      <TranscriptView
        busy={false}
        canBuildPlan={false}
        checkpoints={checkpoints}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        onRestoreCheckpoint={onRestoreCheckpoint}
        timeline={timeline}
      />,
    );

    const transcript = screen.getByLabelText("active-session");
    const checkpointButton = screen.getByTestId("checkpoint-marker-button");
    expect(transcript.textContent).toContain("first prompt");
    expect(transcript.textContent).toContain("Restore Checkpoint");
    expect(transcript.textContent).toContain("second prompt");

    checkpointButton.click();
    expect(onRestoreCheckpoint).toHaveBeenCalledWith("ck-1");
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
        args: { plan_id: "demo-plan" },
        assistantMessageId: "assistant-plan",
        id: "tool-plan",
        isError: false,
        status: "streaming",
        summary: "Creating plan",
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
    expect(screen.queryByTestId("tool-row")).toBeNull();
    expect(screen.getByTestId("plan-card-title").textContent).toBe("Demo plan");
    expect(screen.getByTestId("view-plan-pending")).toBeTruthy();
  });

  it("suppresses a standalone running create_plan tool when the plan card carries the workflow", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        args: { plan_id: "mini-plan" },
        assistantMessageId: "assistant-plan-standalone",
        id: "tool-plan-only",
        isError: false,
        status: "streaming",
        summary: "Creating plan",
        toolCallId: "tc-plan-only",
        toolName: "create_plan",
        type: "tool",
      },
      {
        id: "plan-card-mini",
        path: "/tmp/mini.plan.md",
        planId: "mini-plan",
        state: "planning",
        title: "Mini plan",
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

    expect(screen.queryByTestId("thinking-group")).toBeNull();
    expect(screen.queryByTestId("tool-row")).toBeNull();
    expect(screen.getByTestId("plan-card-title").textContent).toBe("Mini plan");
    expect(screen.getByTestId("view-plan-pending")).toBeTruthy();
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

  it("does not keep the previous thinking block streaming when a new busy turn has no thinking yet", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "first prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1-thinking",
        summaryTitle: null,
        text: "older reasoning",
        type: "thinking",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "older answer",
        type: "message",
      },
      {
        id: "user-2",
        kind: "user",
        text: "latest prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-2",
        id: "assistant-2",
        kind: "assistant",
        text: "new turn has started",
        type: "message",
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

    expect(screen.getByTestId("live-cluster")).toBeTruthy();
    expect(screen.getByTestId("thinking-status").className).not.toContain("tc-codicon-spin");
    expect(screen.queryByTestId("thinking-streaming-indicator")).toBeNull();
  });

  it("does not shimmer a leading thinking group when the live cluster has no thinking yet", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "first prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1-thinking",
        summaryTitle: null,
        text: "inspect the file first",
        type: "thinking",
      },
      {
        args: { path: "/workspace/a.ts" },
        assistantMessageId: "assistant-1",
        id: "tool-read-1",
        isError: false,
        status: "complete",
        summary: "const answer = 42;",
        toolCallId: "tc-read-1",
        toolName: "read",
        type: "tool",
      },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "older answer",
        type: "message",
      },
      {
        id: "user-2",
        kind: "user",
        text: "latest prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-2",
        id: "assistant-2",
        kind: "assistant",
        text: "new turn has started",
        type: "message",
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

    expect(screen.getByTestId("live-cluster")).toBeTruthy();
    expect(screen.getByTestId("thinking-group-title").className).not.toContain(
      "tc-thinking__title--shimmer",
    );
    expect(screen.getByTestId("thinking-group-status").className).not.toContain(
      "codicon-loading",
    );
  });
});
