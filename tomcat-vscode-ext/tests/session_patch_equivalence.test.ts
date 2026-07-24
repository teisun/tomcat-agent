import { describe, expect, it } from "vitest";

import { mergeSessionViewSnapshot, reconcileStateSnapshot } from "../gui/src/stateReconcile";
import { applySessionPatchFrame } from "../gui/src/statePatch";
import type { HostEventFrameContent, WebviewStateSnapshot } from "../src/ui/webview/protocol";
import { WebviewStateStore } from "../src/ui/webview/state";

const SESSION_ID = "s1";

type ScenarioState = {
  activeToolCallId: string | null;
  activeToolName: "bash" | "edit" | "read" | null;
  assistantCounter: number;
  backgroundTaskId: string | null;
  currentAssistantId: string | null;
  lastToolCallId: string | null;
};

function createStore(): WebviewStateStore {
  const store = new WebviewStateStore();
  store.syncSessionList({
    activeSessionId: SESSION_ID,
    scope: "live",
    sessions: [
      {
        busy: false,
        isCurrent: true,
        sessionId: SESSION_ID,
        title: "Session 1",
        updatedAt: 1,
      },
    ],
  });
  store.setActiveSession(SESSION_ID);
  store.setReady(true);
  store.applySessionState({
    busy: false,
    contextRatio: null,
    interrupted: false,
    model: "gpt-5.4",
    planId: "plan-1",
    planPath: "/workspace/plan-1.plan.md",
    planState: "planning",
    sessionId: SESSION_ID,
    thinkingLevel: "high",
  });
  return store;
}

function createRng(seed: number): () => number {
  let value = seed >>> 0;
  return () => {
    value = (value * 1_664_525 + 1_013_904_223) >>> 0;
    return value / 0x1_0000_0000;
  };
}

function nextAssistantId(state: ScenarioState, step: number): string {
  if (!state.currentAssistantId || step % 6 === 0) {
    state.assistantCounter += 1;
    state.currentAssistantId = `assistant-${state.assistantCounter}`;
  }
  return state.currentAssistantId;
}

function makeToolArgs(toolName: "bash" | "edit" | "read", step: number): Record<string, unknown> {
  switch (toolName) {
    case "bash":
      return { command: `echo step-${step}` };
    case "edit":
      return { file: `/workspace/file-${step}.ts` };
    case "read":
      return { path: `/workspace/file-${step}.ts` };
  }
}

type EventBuilder = (state: ScenarioState, step: number) => HostEventFrameContent | null;

const BUILDERS: EventBuilder[] = [
  (state, step) => ({
    assistantMessageEvent: {
      delta: `content-${step}`,
      kind: "content_delta",
    },
    assistantMessageId: nextAssistantId(state, step),
    message: {},
    sessionId: SESSION_ID,
    type: "message_update",
  }),
  (state, step) => ({
    assistantMessageEvent: {
      delta: `thinking-${step}`,
      kind: "thinking_delta",
    },
    assistantMessageId: nextAssistantId(state, step),
    message: {},
    sessionId: SESSION_ID,
    type: "message_update",
  }),
  (state, step) => {
    if (state.activeToolCallId) {
      return null;
    }
    const toolName = (["bash", "read", "edit"] as const)[step % 3];
    const toolCallId = `tool-${step}`;
    state.activeToolCallId = toolCallId;
    state.activeToolName = toolName;
    state.lastToolCallId = toolCallId;
    return {
      args: makeToolArgs(toolName, step),
      sessionId: SESSION_ID,
      toolCallId,
      toolName,
      type: "tool_execution_start",
    };
  },
  (state, step) => {
    if (!state.activeToolCallId || !state.activeToolName) {
      return null;
    }
    return {
      argsPreview: makeToolArgs(state.activeToolName, step),
      sessionId: SESSION_ID,
      toolCallId: state.activeToolCallId,
      toolName: state.activeToolName,
      type: "tool_call_streaming",
    };
  },
  (state, step) => {
    if (!state.activeToolCallId || !state.activeToolName) {
      return null;
    }
    const toolCallId = state.activeToolCallId;
    const toolName = state.activeToolName;
    let result: string;
    if (toolName === "bash" && step % 4 === 0) {
      const taskId = `task-${step}`;
      state.backgroundTaskId = taskId;
      result = JSON.stringify({
        logPath: `/tmp/${taskId}.log`,
        next: "poll task_output",
        startedAtUnixMs: 1_752_000_000_000 + step,
        taskId,
      });
    } else {
      result = `result-${step}`;
    }
    state.activeToolCallId = null;
    state.activeToolName = null;
    return {
      isError: false,
      result,
      sessionId: SESSION_ID,
      toolCallId,
      toolName,
      type: "tool_execution_end",
    };
  },
  (state, step) => {
    if (!state.lastToolCallId) {
      return null;
    }
    return {
      sessionId: SESSION_ID,
      summaryTitle: `Tool summary ${step}`,
      toolCallId: state.lastToolCallId,
      type: "tool.summary_updated",
    };
  },
  (state, step) => {
    if (!state.currentAssistantId && !state.lastToolCallId) {
      return null;
    }
    return {
      assistantMessageId: state.currentAssistantId ?? undefined,
      sessionId: SESSION_ID,
      summaryTitle: `Turn summary ${step}`,
      toolCallIds: state.lastToolCallId ? [state.lastToolCallId] : [],
      turnIndex: step,
      type: "turn.summary_updated",
    };
  },
  (state, step) => {
    if (!state.backgroundTaskId) {
      return null;
    }
    const taskId = state.backgroundTaskId;
    state.backgroundTaskId = null;
    return {
      exitCode: step % 2,
      sessionId: SESSION_ID,
      taskId,
      type: "background_task_finished",
    };
  },
  (_state, step) => ({
    compactionCount: step % 3,
    compactionTokensFreed: step * 10,
    contextUtilizationRatio: ((step % 10) + 1) / 10,
    inputTokensUsed: step * 100,
    preheatInProgress: step % 5 === 0,
    preheatResultPending: step % 7 === 0,
    sessionId: SESSION_ID,
    totalToolResultBytesPersisted: step * 64,
    type: "context_metrics_update",
  }),
  (_state, step) => ({
    sessionId: SESSION_ID,
    title: `Session ${step}`,
    type: "session.title_updated",
  }),
  (_state, step) => ({
    sessionId: SESSION_ID,
    todos: [
      {
        content: `session todo ${step}`,
        id: `session-todo-${step}`,
        status: step % 2 === 0 ? "completed" : "pending",
      },
    ],
    type: "session.todos",
  }),
  (_state, step) => ({
    planId: "plan-1",
    sessionId: SESSION_ID,
    todos: [
      {
        content: `plan todo ${step}`,
        id: `plan-todo-${step}`,
        status: step % 3 === 0 ? "in_progress" : "pending",
      },
    ],
    type: "plan.todos",
  }),
];

function applyIncrementalMutation(
  previous: WebviewStateSnapshot,
  nextStore: WebviewStateStore,
  mutation: ReturnType<WebviewStateStore["applyEvent"]>,
): WebviewStateSnapshot {
  if (mutation.kind === "none") {
    return previous;
  }
  if (mutation.kind === "session") {
    const view = nextStore.snapshotSession(mutation.sessionId);
    if (!view) {
      throw new Error(`Missing session snapshot for ${mutation.sessionId}`);
    }
    return mergeSessionViewSnapshot(previous, {
      sessionId: mutation.sessionId,
      tab: nextStore.snapshotSessionTab(mutation.sessionId) ?? undefined,
      view,
    });
  }
  const patched = applySessionPatchFrame(previous, {
    ops: mutation.ops,
    sessionId: mutation.sessionId,
  });
  if (!patched.ok) {
    throw new Error(patched.error);
  }
  return patched.state;
}

function pickEvent(
  rng: () => number,
  state: ScenarioState,
  step: number,
): HostEventFrameContent {
  const start = Math.floor(rng() * BUILDERS.length);
  for (let offset = 0; offset < BUILDERS.length; offset += 1) {
    const candidate = BUILDERS[(start + offset) % BUILDERS.length](state, step);
    if (candidate) {
      return candidate;
    }
  }
  throw new Error(`No event builder could produce step ${step}`);
}

function runScenario(seed: number, steps: number): void {
  const rng = createRng(seed);
  const store = createStore();
  const scenario: ScenarioState = {
    activeToolCallId: null,
    activeToolName: null,
    assistantCounter: 0,
    backgroundTaskId: null,
    currentAssistantId: null,
    lastToolCallId: null,
  };

  let fullState = reconcileStateSnapshot(undefined, store.snapshot());
  let incrementalState = reconcileStateSnapshot(undefined, store.snapshot());

  for (let step = 1; step <= steps; step += 1) {
    const event = pickEvent(rng, scenario, step);
    const mutation = store.applyEvent(event);
    fullState = reconcileStateSnapshot(fullState, store.snapshot());
    incrementalState = applyIncrementalMutation(incrementalState, store, mutation);
    expect(
      JSON.stringify(incrementalState),
      `seed=${seed} step=${step} event=${event.type}`,
    ).toBe(JSON.stringify(fullState));
  }
}

describe("session patch equivalence", () => {
  it("keeps incremental patch/session merges byte-equivalent to full snapshots across random event sequences", () => {
    for (const seed of [1, 7, 13, 29, 61]) {
      runScenario(seed, 40);
    }
  });
});
