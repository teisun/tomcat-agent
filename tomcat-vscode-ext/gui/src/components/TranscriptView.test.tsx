import { fireEvent, render, screen, within } from "@testing-library/react";
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

  it("keeps assistant rich rendering while showing thinking as plain text", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-rich",
        id: "assistant-msg-rich",
        kind: "assistant",
        text: [
          "Check `src/body/keep.ts:3`.",
          "",
          "```ts src/body/keep.ts:7",
          "export const value = 42;",
          "```",
        ].join("\n"),
        type: "message",
      },
      {
        assistantMessageId: "assistant-rich",
        id: "think-rich",
        summaryTitle: "Reviewed 1 file",
        text: "## Inspect\n\nStart with `src/thinking/plain.ts:9`.",
        type: "thinking",
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

    const assistantMessage = screen.getByTestId("message-block");
    expect(within(assistantMessage).getByTestId("assistant-code-card")).toBeTruthy();
    expect(within(assistantMessage).getByTestId("assistant-clickable-path")).toBeTruthy();

    expect(screen.queryByTestId("thinking-body")).toBeNull();
    fireEvent.click(screen.getByTestId("thinking-toggle"));
    const thinkingBody = screen.getByTestId("thinking-body");
    expect(thinkingBody.tagName).toBe("PRE");
    expect(thinkingBody.textContent).toContain("## Inspect");
    expect(thinkingBody.querySelector("[data-testid='assistant-clickable-path']")).toBeNull();
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

  it("lifts update_plan out of the thinking group into a standalone event row", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-update",
        id: "think-update",
        summaryTitle: "Updated plan for transcript rendering",
        text: "I should check off the next execution step.",
        type: "thinking",
      },
      {
        args: {
          ops: [{ kind: "set_status", status: "completed", todo_id: "todo-2" }],
          path: "/tmp/demo.plan.md",
          plan_id: "demo-plan",
        },
        assistantMessageId: "assistant-update",
        id: "tool-update",
        isError: false,
        planActivity: {
          applied: 1,
          checked: 1,
          completed: 2,
          kind: "update",
          total: 4,
        },
        planId: "demo-plan",
        planPath: "/tmp/demo.plan.md",
        status: "complete",
        summary: "{\"applied\":1}",
        toolCallId: "tc-update",
        toolName: "update_plan",
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
    expect(screen.getByTestId("thinking-group-title").textContent).toBe(
      "Updated plan for transcript rendering",
    );
    expect(screen.getByTestId("tool-row-label").textContent).toContain("Checked 1 · 2/4");
  });

  it("renders a completed create_plan as the single visible plan card for the turn", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-plan",
        id: "think-plan",
        summaryTitle: "Created plan for transcript cleanup",
        text: "Let me break the work down first.",
        type: "thinking",
      },
      {
        args: {
          goal: "Transcript cleanup",
          path: "/tmp/mini.plan.md",
          plan_id: "mini-plan",
          todos: [
            { content: "Audit the transcript path", id: "todo-1", status: "pending" },
            { content: "Render update_plan events", id: "todo-2", status: "pending" },
          ],
        },
        assistantMessageId: "assistant-plan",
        id: "tool-plan-only",
        isError: false,
        planActivity: {
          completed: 0,
          kind: "create",
          stateAfter: "planning",
          title: "Transcript cleanup",
          total: 2,
        },
        planId: "mini-plan",
        planPath: "/tmp/mini.plan.md",
        status: "complete",
        summary: "{\"plan_id\":\"mini-plan\",\"path\":\"/tmp/mini.plan.md\",\"state\":\"planning\"}",
        toolCallId: "tc-plan-only",
        toolName: "create_plan",
        type: "tool",
      },
    ];

    render(
      <TranscriptView
        busy={false}
        canBuildPlan
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        planId="mini-plan"
        planState="planning"
        planTodos={[
          { content: "Audit the transcript path", id: "todo-1", status: "pending" },
          { content: "Render update_plan events", id: "todo-2", status: "pending" },
        ]}
        timeline={timeline}
      />,
    );

    expect(screen.getByTestId("thinking-group")).toBeTruthy();
    expect(screen.getByTestId("plan-card-title").textContent).toBe("Transcript cleanup");
    expect(screen.getByTestId("plan-card-file-name").textContent).toBe("mini.plan.md");
    expect(screen.queryByTestId("tool-row")).toBeNull();
  });

  it("keeps exactly one visible plan card while create_plan flips from running to complete", () => {
    const runningTimeline: WebviewTimelineItem[] = [
      {
        assistantMessageId: "assistant-plan",
        id: "think-plan",
        summaryTitle: "Created plan for transcript cleanup",
        text: "Let me break the work down first.",
        type: "thinking",
      },
      {
        args: {
          draft: "Keep one create card and many update rows.",
          goal: "Transcript cleanup",
          todos: [
            { content: "Audit the transcript path", id: "todo-1", status: "pending" },
            { content: "Render update_plan events", id: "todo-2", status: "pending" },
          ],
        },
        assistantMessageId: "assistant-plan",
        id: "tool-plan-only",
        isError: false,
        planId: "mini-plan",
        planPath: "/tmp/mini.plan.md",
        status: "running",
        toolCallId: "tc-plan-only",
        toolName: "create_plan",
        type: "tool",
      },
    ];
    const completedTimeline: WebviewTimelineItem[] = [
      runningTimeline[0],
      {
        ...runningTimeline[1],
        planActivity: {
          completed: 0,
          kind: "create",
          stateAfter: "planning",
          title: "Transcript cleanup",
          total: 2,
        },
        status: "complete",
        summary: "{\"plan_id\":\"mini-plan\",\"path\":\"/tmp/mini.plan.md\",\"state\":\"planning\"}",
      },
    ];

    const { rerender } = render(
      <TranscriptView
        busy
        canBuildPlan
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        planId="mini-plan"
        planState="planning"
        planTodos={[
          { content: "Audit the transcript path", id: "todo-1", status: "pending" },
          { content: "Render update_plan events", id: "todo-2", status: "pending" },
        ]}
        timeline={runningTimeline}
      />,
    );

    expect(screen.getAllByTestId("plan-card")).toHaveLength(1);
    expect(screen.getByTestId("view-plan-pending")).toBeTruthy();
    expect(screen.queryByTestId("tool-row")).toBeNull();

    rerender(
      <TranscriptView
        busy={false}
        canBuildPlan
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        planId="mini-plan"
        planState="planning"
        planTodos={[
          { content: "Audit the transcript path", id: "todo-1", status: "pending" },
          { content: "Render update_plan events", id: "todo-2", status: "pending" },
        ]}
        timeline={completedTimeline}
      />,
    );

    expect(screen.getAllByTestId("plan-card")).toHaveLength(1);
    expect(screen.queryByTestId("view-plan-pending")).toBeNull();
    expect(screen.getByTestId("view-plan").textContent).toBe("View Plan");
  });

  it("ignores legacy type plan timeline items once create_plan cards own the transcript", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "legacy-plan-card",
        path: "/tmp/legacy.plan.md",
        planId: "legacy-plan",
        state: "planning",
        title: "Legacy plan",
        type: "plan",
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

    expect(screen.queryByTestId("plan-card")).toBeNull();
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

  it("shows a progress row during the pre-stream gap and hides it after the first live item", () => {
    const userOnlyTimeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "latest prompt",
        type: "message",
      },
    ];
    const { rerender } = render(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={userOnlyTimeline}
      />,
    );

    expect(screen.getByTestId("live-cluster")).toBeTruthy();
    expect(screen.getByTestId("progress-row-dots").textContent).toBe("...");
    expect(screen.queryByTestId("progress-row-label")).toBeNull();
    expect(screen.queryByText("Thinking")).toBeNull();

    rerender(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={[
          ...userOnlyTimeline,
          {
            assistantMessageId: "assistant-1",
            id: "assistant-1",
            kind: "assistant",
            text: "first streamed token",
            type: "message",
          },
        ]}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();
  });

  it("reuses the progress row after tools finish until the next live output arrives", () => {
    const userMessage: WebviewTimelineItem = {
      id: "user-1",
      kind: "user",
      text: "latest prompt",
      type: "message",
    };
    const thinkingBlock: WebviewTimelineItem = {
      assistantMessageId: "assistant-1",
      id: "thinking-1",
      summaryTitle: "Used 1 tool",
      text: "checked the README before answering",
      type: "thinking",
    };
    const runningTool: WebviewTimelineItem = {
      args: { path: "README.md" },
      assistantMessageId: "assistant-1",
      id: "tool-1",
      isError: false,
      status: "running",
      summary: "partial read",
      toolCallId: "tool-call-1",
      toolName: "read",
      type: "tool",
    };
    const completedTool: WebviewTimelineItem = {
      ...runningTool,
      status: "complete",
      summary: "# README",
    };

    const { rerender } = render(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={[userMessage, thinkingBlock, runningTool]}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();

    rerender(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={[userMessage, thinkingBlock, completedTool]}
      />,
    );

    expect(screen.getByTestId("progress-row-dots").textContent).toBe("...");
    expect(screen.queryByText("Thinking")).toBeNull();
    expect(screen.getByTestId("thinking-group-title").textContent).toBe("Used 1 tool");
    expect(screen.getByTestId("thinking-group-title").className).not.toContain(
      "tc-thinking__title--shimmer",
    );

    rerender(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={[
          userMessage,
          {
            ...thinkingBlock,
            summaryTitle: "Used 1 tool for checking the README",
          },
          completedTool,
        ]}
      />,
    );

    expect(screen.getByTestId("progress-row-dots").textContent).toBe("...");
    expect(screen.getByTestId("thinking-group-title").textContent).toBe(
      "Used 1 tool for checking the README",
    );

    rerender(
      <TranscriptView
        busy
        canBuildPlan={false}
        onAnswer={vi.fn()}
        onBuildPlan={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPlanFile={vi.fn()}
        timeline={[
          userMessage,
          {
            ...thinkingBlock,
            summaryTitle: "Used 1 tool for checking the README",
          },
          completedTool,
          {
            assistantMessageId: "assistant-1",
            id: "assistant-1-message",
            kind: "assistant",
            text: "The README looks good.",
            type: "message",
          },
        ]}
      />,
    );

    expect(screen.queryByTestId("progress-row")).toBeNull();
  });

  it("only shimmers thinking groups that still have unfinished work in the live cluster", () => {
    const timeline: WebviewTimelineItem[] = [
      {
        id: "user-1",
        kind: "user",
        text: "latest prompt",
        type: "message",
      },
      {
        assistantMessageId: "assistant-complete",
        id: "thinking-complete",
        summaryTitle: "Reviewed 1 file",
        text: "checked the current file",
        type: "thinking",
      },
      {
        args: { path: "/workspace/finished.ts" },
        assistantMessageId: "assistant-complete",
        id: "tool-complete",
        isError: false,
        status: "complete",
        summary: "export const done = true;",
        toolCallId: "tc-complete",
        toolName: "read",
        type: "tool",
      },
      {
        assistantMessageId: "assistant-running",
        id: "thinking-running",
        summaryTitle: "Used 2 tools",
        text: "now checking the remaining references",
        type: "thinking",
      },
      {
        args: { path: "/workspace/active.ts" },
        assistantMessageId: "assistant-running",
        id: "tool-running-1",
        isError: false,
        status: "running",
        summary: "partial contents",
        toolCallId: "tc-running-1",
        toolName: "read",
        type: "tool",
      },
      {
        args: { query: "activeSymbol" },
        assistantMessageId: "assistant-running",
        id: "tool-running-2",
        isError: false,
        status: "complete",
        summary: "Found 1 result.\nactive.ts:3",
        toolCallId: "tc-running-2",
        toolName: "search_workspace",
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

    const titles = screen.getAllByTestId("thinking-group-title");
    const statuses = screen.getAllByTestId("thinking-group-status");

    expect(titles).toHaveLength(2);
    expect(titles[0].className).not.toContain("tc-thinking__title--shimmer");
    expect(titles[1].className).toContain("tc-thinking__title--shimmer");
    expect(titles[1].textContent).toBe("Used 2 tools");

    expect(statuses[0].className).toContain("codicon-search");
    expect(statuses[1].className).toContain("codicon-search");
    expect(statuses[0].className).not.toContain("codicon-loading");
    expect(statuses[1].className).not.toContain("codicon-loading");
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
    expect(screen.getByTestId("thinking-group-status").className).toContain("codicon-search");
  });
});
