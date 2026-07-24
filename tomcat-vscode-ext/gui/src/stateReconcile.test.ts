import { describe, expect, it } from "vitest";

import type {
  WebviewSessionSnapshot,
  WebviewSessionTab,
  WebviewStateSnapshot,
} from "./types";
import {
  mergeSessionViewSnapshot,
  reconcileStateSnapshot,
} from "./stateReconcile";

function sessionTab(sessionId: string, overrides: Partial<WebviewSessionTab> = {}): WebviewSessionTab {
  return {
    busy: false,
    isCurrent: sessionId === "s1",
    ownedByThisFrontend: true,
    sessionId,
    title: sessionId,
    updatedAt: 1,
    ...overrides,
  };
}

function session(
  sessionId: string,
  timelineText: string[],
  overrides: Partial<WebviewSessionSnapshot> = {},
): WebviewSessionSnapshot {
  return {
    busy: false,
    checkpoints: [],
    contextRatio: null,
    hasMoreHistory: false,
    historyLoading: false,
    model: "gpt-5.4",
    ownedByThisFrontend: true,
    pendingAttachments: [],
    planFile: null,
    planId: null,
    planState: "chat",
    planTodos: [],
    sessionId,
    sessionTodos: [],
    thinkingLevel: "high",
    timeline: timelineText.map((text, index) => ({
      id: `${sessionId}-m${index + 1}`,
      kind: index % 2 === 0 ? "user" : "assistant",
      text,
      type: "message" as const,
    })),
    ...overrides,
  };
}

function snapshot(): WebviewStateSnapshot {
  return {
    activeSessionId: "s1",
    availableModelCapabilities: {},
    availableModelReasoningLevels: {},
    availableModels: ["gpt-5.4"],
    modelAdminSupported: false,
    ready: true,
    sessions: [sessionTab("s1"), sessionTab("s2")],
    sessionViews: {
      s1: session("s1", ["hello", "world"]),
      s2: session("s2", ["other"]),
    },
  };
}

function review(id: string, summary = "Looks good") {
  return {
    findings: [],
    id,
    planId: "plan-1",
    reviewAttemptId: `${id}:attempt-1`,
    status: "done" as const,
    summary,
    type: "review" as const,
    verdict: "pass" as const,
  };
}

describe("stateReconcile", () => {
  it("returns the previous state when the full snapshot is structurally unchanged", () => {
    const previous = snapshot();
    const reconciled = reconcileStateSnapshot(previous, snapshot());

    expect(reconciled).toBe(previous);
  });

  it("reuses unchanged timeline entries and untouched sessions", () => {
    const previous = snapshot();
    const next = snapshot();
    next.sessionViews.s1.timeline[1] = {
      ...next.sessionViews.s1.timeline[1],
      text: "updated",
    };

    const reconciled = reconcileStateSnapshot(previous, next);

    expect(reconciled.sessionViews.s1.timeline[0]).toBe(
      previous.sessionViews.s1.timeline[0],
    );
    expect(reconciled.sessionViews.s1.timeline[1]).not.toBe(
      previous.sessionViews.s1.timeline[1],
    );
    expect(reconciled.sessionViews.s2).toBe(previous.sessionViews.s2);
    expect(reconciled.sessions).toBe(previous.sessions);
  });

  it("only swaps the streaming tail reference when the tail text changes", () => {
    const previous = snapshot();
    const next = snapshot();
    next.sessionViews.s1.timeline.push({
      id: "s1-m3",
      kind: "assistant",
      text: "tail",
      type: "message",
    });
    const previousWithTail = reconcileStateSnapshot(previous, next);

    const nextDelta = snapshot();
    nextDelta.sessionViews.s1.timeline.push({
      id: "s1-m3",
      kind: "assistant",
      text: "tail + delta",
      type: "message",
    });

    const reconciled = reconcileStateSnapshot(previousWithTail, nextDelta);

    expect(reconciled.sessionViews.s1.timeline[0]).toBe(
      previousWithTail.sessionViews.s1.timeline[0],
    );
    expect(reconciled.sessionViews.s1.timeline[1]).toBe(
      previousWithTail.sessionViews.s1.timeline[1],
    );
    expect(reconciled.sessionViews.s1.timeline[2]).not.toBe(
      previousWithTail.sessionViews.s1.timeline[2],
    );
  });

  it("preserves surviving references when entries are removed", () => {
    const previous = snapshot();
    previous.sessionViews.s1.timeline.push({
      id: "s1-m3",
      kind: "assistant",
      text: "tail",
      type: "message",
    });
    const removed = snapshot();
    removed.sessionViews.s1.timeline = [
      removed.sessionViews.s1.timeline[0],
      {
        id: "s1-m3",
        kind: "assistant",
        text: "tail",
        type: "message",
      },
    ];

    const reconciled = reconcileStateSnapshot(previous, removed);

    expect(reconciled.sessionViews.s1.timeline).toHaveLength(2);
    expect(reconciled.sessionViews.s1.timeline[0]).toBe(
      previous.sessionViews.s1.timeline[0],
    );
    expect(reconciled.sessionViews.s1.timeline[1]).toBe(
      previous.sessionViews.s1.timeline[2],
    );
  });

  it("uses type-prefixed keys so review rows do not collide with other item kinds", () => {
    const previous = snapshot();
    previous.sessionViews.s1.timeline = [
      {
        id: "shared-1",
        kind: "assistant",
        text: "message",
        type: "message",
      },
      review("shared-1"),
    ];
    const next = snapshot();
    next.sessionViews.s1.timeline = [
      {
        id: "shared-1",
        kind: "assistant",
        text: "message",
        type: "message",
      },
      review("shared-1"),
    ];

    const reconciled = reconcileStateSnapshot(previous, next);

    expect(reconciled.sessionViews.s1.timeline[0]).toBe(
      previous.sessionViews.s1.timeline[0],
    );
    expect(reconciled.sessionViews.s1.timeline[1]).toBe(
      previous.sessionViews.s1.timeline[1],
    );
  });

  it("merges one sessionView without disturbing sibling sessions or tabs", () => {
    const previous = snapshot();
    const nextSession = session("s2", ["other", "delta"], { busy: true });
    const nextTab = sessionTab("s2", { busy: true, updatedAt: 2 });

    const merged = mergeSessionViewSnapshot(previous, {
      sessionId: "s2",
      tab: nextTab,
      view: nextSession,
    });

    expect(merged.sessionViews.s1).toBe(previous.sessionViews.s1);
    expect(merged.sessions[0]).toBe(previous.sessions[0]);
    expect(merged.sessionViews.s2).not.toBe(previous.sessionViews.s2);
    expect(merged.sessions[1]).not.toBe(previous.sessions[1]);
    expect(merged.sessionViews.s2.timeline).toHaveLength(2);
    expect(merged.sessions[1]).toMatchObject({ busy: true, updatedAt: 2 });
  });

  it("returns the previous state when a sessionView is structurally unchanged", () => {
    const previous = snapshot();
    const merged = mergeSessionViewSnapshot(previous, {
      sessionId: "s1",
      tab: sessionTab("s1"),
      view: session("s1", ["hello", "world"]),
    });

    expect(merged).toBe(previous);
  });
});
