import * as assert from "node:assert/strict";
import { execSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";

import * as vscode from "vscode";

import {
  EXTENSION_ID,
  TOMCAT_ANSWER_COMMAND,
} from "../../../constants";
import type { PendingQuestionSnapshot } from "../../../ui/participant/commands";
import type {
  ObservedEventFilter,
  TomcatExtensionApi,
  WebviewIntent,
} from "../../../extension";

let dummyLanguageModelRegistration: vscode.Disposable | undefined;
let hasWarmedChatUi = false;
type LanguageModelRegistry = {
  registerLanguageModelChatProvider(
    vendor: string,
    provider: {
      provideLanguageModelChatInformation(
        options: unknown,
        token: vscode.CancellationToken,
      ): vscode.ProviderResult<unknown[]>;
      provideLanguageModelChatResponse(
        model: unknown,
        messages: readonly unknown[],
        options: unknown,
        progress: vscode.Progress<unknown>,
        token: vscode.CancellationToken,
      ): Thenable<void>;
      provideTokenCount(
        model: unknown,
        text: string | unknown,
        token: vscode.CancellationToken,
      ): Thenable<number>;
    },
  ): vscode.Disposable;
};

function requireEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be defined for host E2E`);
  return value;
}

function collectStreamText(
  stream: Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"],
  kind: "markdown" | "progress",
): string {
  return stream
    .flatMap((event) =>
      event.kind === kind ? [event.value] : [],
    )
    .join("\n");
}

function getButton(
  stream: Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"],
  title: string,
): Extract<
  Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"][number],
  { kind: "button" }
> {
  const button = stream.find(
    (event): event is Extract<(typeof stream)[number], { kind: "button" }> =>
      event.kind === "button" && event.title === title,
  );
  assert.ok(button, `expected stream button ${title}`);
  return button;
}

export async function getTomcatExtensionApi(): Promise<TomcatExtensionApi> {
  if (!dummyLanguageModelRegistration) {
    const registry = vscode.lm as unknown as LanguageModelRegistry;
    dummyLanguageModelRegistration = registry.registerLanguageModelChatProvider(
      "tomcat-test",
      {
        provideLanguageModelChatInformation: async () => [
          {
            capabilities: {},
            family: "test",
            id: "tomcat-e2e-model",
            isDefault: true,
            isUserSelectable: true,
            maxInputTokens: 4_096,
            maxOutputTokens: 4_096,
            name: "tomcat-e2e-model",
            version: "1.0.0",
          },
        ],
        provideLanguageModelChatResponse: async () => undefined,
        provideTokenCount: async () => 1,
      },
    );
  }

  const extension = vscode.extensions.getExtension<TomcatExtensionApi>(
    EXTENSION_ID,
  );

  assert.ok(extension, "expected Tomcat extension to be discoverable");
  const exports = await extension.activate();
  assert.ok(extension.isActive, "expected Tomcat extension to activate");
  await new Promise((resolve) => setTimeout(resolve, 2_000));
  return exports;
}

async function startChatQuery(
  query: string,
  options: {
    blockOnResponse?: boolean;
    newChat?: boolean;
  } = {},
): Promise<{ metadata?: { sessionId?: string } } | undefined> {
  if (options.newChat) {
    await vscode.commands.executeCommand(
      "workbench.action.chat.triggerSetupAnonymousWithoutDialog",
    );
    await vscode.commands.executeCommand(
      "workbench.action.chat.newChat",
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  return new Promise<{ metadata?: { sessionId?: string } } | undefined>(
    (resolve, reject) => {
      setTimeout(() => {
        void vscode.commands
          .executeCommand<{ metadata?: { sessionId?: string } } | undefined>(
            "workbench.action.chat.open",
            {
              blockOnResponse: options.blockOnResponse ?? false,
              mode: "ask",
              query,
            },
          )
          .then(resolve, reject);
      }, 0);
    },
  );
}

async function warmChatUi(): Promise<void> {
  if (hasWarmedChatUi) {
    return;
  }

  hasWarmedChatUi = true;
  try {
    await startChatQuery("@tomcat warm up", {
      newChat: true,
    });
    await new Promise((resolve) => setTimeout(resolve, 1_000));
  } catch {
    // The warm-up request is best-effort; later assertions exercise the real checks.
  }
}

async function waitForEvent(
  api: TomcatExtensionApi,
  filter: ObservedEventFilter,
): Promise<void> {
  await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    ...filter,
  });
}

async function waitForSessionState<T>(
  api: TomcatExtensionApi,
  predicate: (state: Awaited<ReturnType<TomcatExtensionApi["__testing"]["getSessionState"]>>) => T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const state = await api.__testing.getSessionState();
    const result = predicate(state);
    if (result !== undefined) {
      return result;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("Timed out waiting for session state to match the expected condition");
}

async function waitForWebviewState<T>(
  api: TomcatExtensionApi,
  predicate: (state: ReturnType<TomcatExtensionApi["__testing"]["getWebviewState"]>) => T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const state = api.__testing.getWebviewState();
    const result = predicate(state);
    if (result !== undefined) {
      return result;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("Timed out waiting for webview state to match the expected condition");
}

async function waitForWebviewDomSnapshot<T>(
  api: TomcatExtensionApi,
  predicate: Awaited<ReturnType<TomcatExtensionApi["__testing"]["captureWebviewDom"]>> extends infer Snapshot
    ? (snapshot: Snapshot) => T | undefined
    : never,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  let lastSnapshot:
    | Awaited<ReturnType<TomcatExtensionApi["__testing"]["captureWebviewDom"]>>
    | undefined;
  while (Date.now() - startedAt < timeoutMs) {
    const snapshot = await api.__testing.captureWebviewDom();
    lastSnapshot = snapshot;
    const result = predicate(snapshot);
    if (result !== undefined) {
      return result;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const dbg = lastSnapshot
    ? {
        activeSessionId: lastSnapshot.activeSessionId,
        assistantResponseGroups: lastSnapshot.assistantResponseGroups,
        groupFoldTitles: lastSnapshot.groupFoldTitles,
        userPromptPill: lastSnapshot.userPromptPill,
        assistantNoCard: lastSnapshot.assistantNoCard,
        planCardCount: lastSnapshot.planCardCount,
        planFooterSameRow: lastSnapshot.planFooterSameRow,
        planCardTodoCountText: lastSnapshot.planCardTodoCountText,
        composerFooterPlanStatus: lastSnapshot.composerFooterPlanStatus,
        composerPlanStatusInBarCount: lastSnapshot.composerPlanStatusInBarCount,
        ctxLabel: lastSnapshot.ctxLabel,
        fileChipTopWithinStream: lastSnapshot.fileChipTopWithinStream,
        fileChipVisible: lastSnapshot.fileChipVisible,
        historyLoaderVisible: lastSnapshot.historyLoaderVisible,
        planNoticeReplayed: lastSnapshot.planNoticeReplayed,
        planStateText: lastSnapshot.planStateText,
        progressRow: lastSnapshot.progressRow,
        planTodos: lastSnapshot.planTodos,
        todoWidgetVisible: lastSnapshot.todoWidgetVisible,
        todoWidgetExpanded: lastSnapshot.todoWidgetExpanded,
        todoWidgetItemCount: lastSnapshot.todoWidgetItemCount,
        todoWidgetTitle: lastSnapshot.todoWidgetTitle,
        toolRowFlat: lastSnapshot.toolRowFlat,
        toolRowExpandable: lastSnapshot.toolRowExpandable,
        ellipsisAboveGroupHeader: lastSnapshot.ellipsisAboveGroupHeader,
        leftGuideLine: lastSnapshot.leftGuideLine,
        sessionTitleUpdated: lastSnapshot.sessionTitleUpdated,
        timelineKinds: lastSnapshot.timelineKinds,
        messageTexts: lastSnapshot.messageTexts,
        toolTitles: lastSnapshot.toolTitles,
        html: (lastSnapshot.html ?? "").slice(0, 4000),
      }
    : undefined;
  throw new Error(
    `Timed out waiting for webview DOM to match the expected condition. lastSnapshot=${JSON.stringify(dbg)}`,
  );
}

async function answerPendingQuestion(
  pending: PendingQuestionSnapshot,
): Promise<void> {
  const question = pending.questions[0];
  const approveOption = question.options[0];
  assert.ok(approveOption, "expected an approval option");
  await vscode.commands.executeCommand(TOMCAT_ANSWER_COMMAND, {
    kind: "direct",
    optionId: approveOption.id,
    pickedRecommended: !!approveOption.recommended,
    questionId: question.id,
    requestId: pending.requestId,
  });
}

function buildWebviewIntent(
  intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>,
): Exclude<WebviewIntent, { type: "__test.dom_snapshot" }> {
  return intent;
}

export async function assertParticipantHappyPath(
  api: TomcatExtensionApi,
): Promise<void> {
  const turn = await api.__testing.runParticipantTurn({
    prompt: "hello fake tomcat",
  });
  const markdown = collectStreamText(turn.stream, "markdown");

  assert.match(markdown, /hello from fake tomcat/i);
  assert.equal(typeof turn.result?.metadata?.sessionId, "string");
}

export async function assertParticipantHappyPathViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  await warmChatUi();
  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat hello fake tomcat", {
    newChat: true,
  });
  await waitForEvent(api, {
    textIncludes: "hello from fake tomcat",
    type: "message_update",
  });
  const completed = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  assert.equal(typeof completed.sessionId, "string");
}

export async function assertApprovalDiffFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const editFile = requireEnv("TOMCAT_VSCODE_TEST_EDIT_FILE");
  await fs.writeFile(editFile, "before\n", "utf8");

  const turn = await api.__testing.runParticipantTurn({
    autoClickTitles: ["Approve"],
    prompt: "approve edit",
  });

  assert.match(collectStreamText(turn.stream, "markdown"), /edit applied/i);
  getButton(turn.stream, "Open Diff");
  const applyButton = getButton(turn.stream, "Apply Edit");
  const toolCallId = (applyButton.arguments?.[0] as { toolCallId?: string } | undefined)
    ?.toolCallId;

  assert.ok(toolCallId, "expected diff/apply button to carry toolCallId");
  const prepared = api.__testing.getPreparedChange(toolCallId);
  assert.ok(prepared, "expected prepared change");
  assert.equal(prepared.originalContent, "before\n");
  assert.equal(prepared.proposedContent, "after\n");

  await api.__testing.openPreparedDiff(toolCallId);
  assert.equal(await api.__testing.applyPreparedEdit(toolCallId), true);
  assert.equal(await fs.readFile(editFile, "utf8"), "after\n");

  await vscode.commands.executeCommand("workbench.action.closeAllEditors");
}

export async function assertApprovalDiffFlowViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  await warmChatUi();
  const editFile = requireEnv("TOMCAT_VSCODE_TEST_EDIT_FILE");
  await fs.writeFile(editFile, "before\n", "utf8");
  api.__testing.clearObservedEvents();

  await startChatQuery("@tomcat approve edit", {
    newChat: true,
  });
  const pending = await api.__testing.waitForPendingQuestion();
  await answerPendingQuestion(pending);

  const completed = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  assert.equal(typeof completed.sessionId, "string");
  await waitForEvent(api, { type: "tool_execution_end" });

  const prepared = api.__testing.getPreparedChange("tool-edit-1");
  assert.ok(prepared, "expected prepared change from real chat UI");
  assert.equal(prepared.originalContent, "before\n");
  assert.equal(prepared.proposedContent, "after\n");

  await api.__testing.openPreparedDiff("tool-edit-1");
  assert.equal(await api.__testing.applyPreparedEdit("tool-edit-1"), true);
  assert.equal(await fs.readFile(editFile, "utf8"), "after\n");

  await vscode.commands.executeCommand("workbench.action.closeAllEditors");
}

export async function assertInterruptAndRestartFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const interrupted = await api.__testing.runParticipantTurn({
    cancelAfterMs: 50,
    prompt: "interrupt please",
  });
  assert.match(
    collectStreamText(interrupted.stream, "progress"),
    /interrupted/i,
  );

  const beforeRestartSessions = await api.__testing.listSessions();
  assert.ok(
    beforeRestartSessions.sessions.length >= 1,
    "expected at least one session before restart",
  );

  await api.__testing.restartServe();

  const afterRestart = await api.__testing.runParticipantTurn({
    prompt: "hello after restart",
  });
  assert.match(
    collectStreamText(afterRestart.stream, "markdown"),
    /hello from fake tomcat/i,
  );
}

export async function assertInterruptAndRestartFlowViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat interrupt please", {
    newChat: true,
  });
  await waitForEvent(api, {
    textIncludes: "partial",
    type: "message_update",
  });

  await vscode.commands.executeCommand("workbench.action.chat.cancel");
  await waitForEvent(api, { type: "agent_interrupted" });
  await waitForEvent(api, {
    textIncludes: "interrupted",
    type: "agent_end",
  });

  await api.__testing.restartServe();
  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat hello after restart", {
    newChat: true,
  });
  await waitForEvent(api, {
    textIncludes: "hello from fake tomcat",
    type: "message_update",
  });
  const completed = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  assert.equal(typeof completed.sessionId, "string");
}

export async function assertMultiSessionRouting(
  api: TomcatExtensionApi,
): Promise<void> {
  const sessionA = await api.__testing.runParticipantTurn({
    prompt: "thread A",
  });
  const sessionAId = sessionA.result?.metadata?.sessionId;
  assert.equal(typeof sessionAId, "string");

  const sessionB = await api.__testing.runParticipantTurn({
    prompt: "thread B",
  });
  const sessionBId = sessionB.result?.metadata?.sessionId;
  assert.equal(typeof sessionBId, "string");
  assert.notEqual(sessionAId, sessionBId);

  const followUpA = await api.__testing.runParticipantTurn({
    historySessionId: sessionAId,
    prompt: "follow up A",
  });
  const followUpB = await api.__testing.runParticipantTurn({
    historySessionId: sessionBId,
    prompt: "follow up B",
  });

  assert.equal(followUpA.result?.metadata?.sessionId, sessionAId);
  assert.equal(followUpB.result?.metadata?.sessionId, sessionBId);

  const sessions = await api.__testing.listSessions();
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionAId),
    "expected session A to remain listed",
  );
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionBId),
    "expected session B to remain listed",
  );
}

export async function assertMultiSessionRoutingViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat thread A", {
    newChat: true,
  });
  const sessionA = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  const sessionAId = sessionA.sessionId;
  assert.equal(typeof sessionAId, "string");

  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat follow up A", {
    newChat: false,
  });
  const followUpA = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  assert.equal(followUpA.sessionId, sessionAId);

  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat thread B", {
    newChat: true,
  });
  const sessionB = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  const sessionBId = sessionB.sessionId;
  assert.equal(typeof sessionBId, "string");
  assert.notEqual(sessionAId, sessionBId);

  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat follow up B", {
    newChat: false,
  });
  const followUpB = await api.__testing.waitForEvent({
    timeoutMs: 15_000,
    type: "agent_end",
  });
  assert.equal(followUpB.sessionId, sessionBId);

  const sessions = await api.__testing.listSessions();
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionAId),
    "expected session A to remain listed after real chat UI",
  );
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionBId),
    "expected session B to remain listed after real chat UI",
  );
}

export async function assertPlanSlashFlowViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  await warmChatUi();
  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat /plan", {
    newChat: true,
  });
  let state = await waitForSessionState(
    api,
    (candidate) => (candidate.planState === "planning" ? candidate : undefined),
  );
  assert.equal(state.planState, "planning");

  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat /plan build fake-plan", {
    newChat: false,
  });
  await waitForEvent(api, { type: "plan.build" });
  await waitForEvent(api, { type: "agent_end" });
  state = await api.__testing.getSessionState();
  assert.equal(state.planState, "executing");
  assert.equal(state.planId, "fake-plan");

  api.__testing.clearObservedEvents();
  await startChatQuery("@tomcat /plan exit", {
    newChat: false,
  });
  state = await waitForSessionState(
    api,
    (candidate) => (candidate.planState === "chat" ? candidate : undefined),
  );
  assert.equal(state.planState, "chat");
}

export async function assertModelSlashFlowViaChatUi(
  api: TomcatExtensionApi,
): Promise<void> {
  await warmChatUi();
  api.__testing.setParticipantUiOverrides({
    showQuickPick: async <T extends vscode.QuickPickItem>(
      items: readonly T[],
    ): Promise<T | undefined> =>
      items.find((item) => item.label === "claude-4.6-sonnet"),
  });
  await startChatQuery("@tomcat /model", {
    newChat: true,
  });
  let state = await waitForSessionState(
    api,
    (candidate) =>
      candidate.model === "claude-4.6-sonnet" ? candidate : undefined,
  );
  assert.equal(state.model, "claude-4.6-sonnet");

  api.__testing.setParticipantUiOverrides({
    showQuickPick: async () => undefined,
  });
  await startChatQuery("@tomcat /model", {
    newChat: false,
  });
  state = await waitForSessionState(
    api,
    (candidate) =>
      candidate.model === "claude-4.6-sonnet" ? candidate : undefined,
  );
  assert.equal(state.model, "claude-4.6-sonnet");

  api.__testing.setParticipantUiOverrides({});
}

export async function assertWebviewStreamingFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        text: "hello fake tomcat",
      },
      messageId: "webview-stream-1",
      type: "prompt",
    }),
  );
  await waitForEvent(api, {
    textIncludes: "hello from fake tomcat",
    type: "message_update",
  });
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.messageTexts.some((text) => /hello from fake tomcat/i.test(text))
        ? candidate
        : undefined,
  );
  assert.ok(
    snapshot.messageTexts.some((text) => /hello from fake tomcat/i.test(text)),
    "expected webview DOM to render the streamed assistant text",
  );
}

export async function assertWebviewInterruptFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        text: "interrupt please",
      },
      messageId: "webview-interrupt-1",
      type: "prompt",
    }),
  );
  await waitForEvent(api, {
    textIncludes: "partial",
    type: "message_update",
  });
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.busy ? activeSessionId : undefined;
    },
    20_000,
  );
  await api.__testing.injectServeEvent({
    args: { path: "src/app.ts" },
    sessionId,
    toolCallId: "interrupt-tool-1",
    toolName: "edit",
    type: "tool_execution_start",
  });
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="stop-button"') &&
      snapshot.html.includes('data-testid="tool-row-running-indicator"')
        ? snapshot
        : undefined,
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: "webview-interrupt-stop",
      type: "interrupt",
    }),
  );
  await waitForEvent(api, { type: "agent_interrupted" });
  await waitForEvent(api, {
    textIncludes: "interrupted",
    type: "agent_end",
  });

  const settled = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="send-button"') &&
      !snapshot.html.includes('data-testid="stop-button"') &&
      !snapshot.html.includes('data-testid="tool-row-running-indicator"') &&
      snapshot.messageTexts.filter((text) => text === "Tomcat turn interrupted").length === 1 &&
      snapshot.html.includes("tc-message--warn")
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    settled.html.includes("Interrupted"),
    "expected interrupted tool rows to render an interrupted summary instead of a spinner",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-interrupt-new-session",
      type: "newSession",
    }),
  );
  const otherSessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId || activeSessionId === sessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
    20_000,
  );
  assert.notEqual(otherSessionId, sessionId);

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: "webview-interrupt-switch-back",
      type: "switchSession",
    }),
  );
  const restored = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="send-button"') &&
      !snapshot.html.includes('data-testid="stop-button"') &&
      !snapshot.html.includes('data-testid="tool-row-running-indicator"') &&
      snapshot.messageTexts.filter((text) => text === "Tomcat turn interrupted").length === 1
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    restored.html.includes("Interrupted"),
    "expected interrupted state to stay restored after switching away and back",
  );
}

export async function assertWebviewAnswerCardFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-answer-card-session",
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "answer card showcase",
      },
      messageId: "webview-answer-card-1",
      type: "prompt",
    }),
  );
  const approval = await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionId];
      if (!session) {
        return undefined;
      }
      const pending = session?.timeline.find(
        (
          item,
        ): item is Extract<typeof session.timeline[number], { type: "approval" }> =>
          item.type === "approval" && !item.resolved,
      );
      return pending ? { pending } : undefined;
    },
    20_000,
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        requestId: approval.pending.request.requestId,
        result: {
          answers: [
            {
              optionIds: ["staging"],
              pickedRecommended: true,
              questionId: approval.pending.request.questions[0].id,
              skipped: false,
            },
          ],
          cancelled: false,
        },
      },
      messageId: "webview-answer-card-approve",
      type: "answerQuestion",
    }),
  );

  await waitForEvent(api, { type: "tool_execution_end" });
  await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.toolTitles.some((title) => /Asked question/i.test(title))
        ? candidate
        : undefined,
    20_000,
  );
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "tool-row-toggle",
  });
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.html.includes('data-testid="answer-card"') &&
      candidate.html.includes("Deploy where?") &&
      candidate.html.includes("Staging")
        ? candidate
        : undefined,
    20_000,
  );
  assert.doesNotMatch(snapshot.html, /"optionIds"\s*:/u);
}

export async function assertWebviewDiffFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const editFile = requireEnv("TOMCAT_VSCODE_TEST_EDIT_FILE");
  await fs.writeFile(editFile, "before\n", "utf8");
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-diff-session",
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "approve edit",
      },
      messageId: "webview-approve-1",
      type: "prompt",
    }),
  );
  const { activeSessionId, approval } = await waitForWebviewState(
    api,
    (state) => {
      for (const [sessionId, session] of Object.entries(state.sessionViews)) {
        const pendingApproval = session.timeline.find(
          (
            item,
          ): item is Extract<typeof session.timeline[number], { type: "approval" }> =>
            item.type === "approval" && !item.resolved,
        );
        if (pendingApproval) {
          return {
            activeSessionId: sessionId,
            approval: pendingApproval,
          };
        }
      }
      return undefined;
    },
  );
  assert.ok(activeSessionId, "expected the webview to have an active session");
  assert.ok(approval, "expected a pending webview approval");
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        requestId: approval.request.requestId,
        result: {
          answers: [
            {
              optionIds: ["approve"],
              pickedRecommended: true,
              questionId: approval.request.questions[0].id,
              skipped: false,
            },
          ],
          cancelled: false,
        },
      },
      messageId: "webview-approve-answer",
      type: "answerQuestion",
    }),
  );

  await waitForEvent(api, { type: "tool_execution_end" });
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === activeSessionId && candidate.toolRowCount > 0
        ? candidate
        : undefined,
    20_000,
  );
  assert.doesNotMatch(snapshot.html, /Open Diff/u);
  assert.doesNotMatch(snapshot.html, /Apply Edit/u);
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { toolCallId: "tool-edit-1" },
      messageId: "webview-open-diff",
      type: "openDiff",
    }),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { toolCallId: "tool-edit-1" },
      messageId: "webview-apply-edit",
      type: "applyEdit",
    }),
  );
  assert.equal(await fs.readFile(editFile, "utf8"), "after\n");
}

export async function assertWebviewMultiSessionFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-new-session-a",
      type: "newSession",
    }),
  );
  const stateA = api.__testing.getWebviewState();
  const sessionA = stateA.activeSessionId;
  assert.ok(sessionA, "expected session A");

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { text: "thread A" },
      messageId: "webview-thread-a",
      type: "prompt",
    }),
  );
  await waitForEvent(api, { sessionId: sessionA!, type: "agent_end" });

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-new-session-b",
      type: "newSession",
    }),
  );
  const stateB = api.__testing.getWebviewState();
  const sessionB = stateB.activeSessionId;
  assert.ok(sessionB, "expected session B");
  assert.notEqual(sessionA, sessionB);

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { text: "thread B" },
      messageId: "webview-thread-b",
      type: "prompt",
    }),
  );
  await waitForEvent(api, { sessionId: sessionB!, type: "agent_end" });

  // SessionBar renders sessions in a collapsed dropdown; the options only
  // exist in the DOM when the dropdown is open. Assert against webview state
  // (the source of truth for multi-session isolation) instead of the DOM.
  const sessions = stateB.sessions.map((tab) => tab.sessionId);
  assert.ok(
    sessions.length >= 2,
    "expected the webview state to track multiple sessions",
  );
  assert.ok(sessions.includes(sessionA!), "expected session A to remain tracked");
  assert.ok(sessions.includes(sessionB!), "expected session B to be tracked");
}

export async function assertWebviewOwnershipFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const participantTurn = await api.__testing.runParticipantTurn({
    prompt: "participant owner",
  });
  const sessionId = participantTurn.result?.metadata?.sessionId;
  assert.equal(typeof sessionId, "string");

  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: "webview-ownership-switch-1",
      type: "switchSession",
    }),
  );
  let state = await waitForWebviewState(
    api,
    (candidate) => {
      const activeSessionId = candidate.activeSessionId;
      if (activeSessionId !== sessionId) {
        return undefined;
      }
      return candidate.sessionViews[sessionId!]?.conflictMessage ? candidate : undefined;
    },
  );
  assert.ok(state.sessionViews[sessionId!]?.conflictMessage);

  assert.equal(api.__testing.releaseSessionOwnership(sessionId!, "participant"), true);
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: "webview-ownership-switch-2",
      type: "switchSession",
    }),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId, text: "hello after release" },
      messageId: "webview-ownership-prompt",
      type: "prompt",
    }),
  );
  await waitForEvent(api, { sessionId, type: "agent_end" });
  state = await waitForWebviewState(
    api,
    (candidate) => {
      const activeSessionId = candidate.activeSessionId;
      if (activeSessionId !== sessionId) {
        return undefined;
      }
      return candidate.sessionViews[sessionId!]?.conflictMessage === null
        ? candidate
        : undefined;
    },
  );
  assert.equal(state.sessionViews[sessionId!]?.conflictMessage, null);
}

export async function assertWebviewSessionSwitchRestoreFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-restore-new-session-a",
      type: "newSession",
    }),
  );
  const sessionA = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId: sessionA,
        text: "transcript ui",
      },
      messageId: "webview-restore-plan-seed",
      type: "prompt",
    }),
  );
  const initial = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionA &&
      snapshot.ctxLabel === "Ctx 55%" &&
      snapshot.planCardCount === 1 &&
      snapshot.planStateText === "Plan: planning"
        ? snapshot
        : undefined,
    20_000,
  );
  assert.match(initial.html, /data-testid="build-plan"/u);
  assert.ok(!initial.disabledTestIds.includes("build-plan"), "expected Build to be enabled");
  assert.ok(
    initial.messageTexts.some((text) => /transcript ui/i.test(text)),
    "expected session A transcript to be visible before switching away",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-restore-new-session-b",
      type: "newSession",
    }),
  );
  const sessionB = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId || activeSessionId === sessionA) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );
  assert.notEqual(sessionA, sessionB);

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionA },
      messageId: "webview-restore-switch-back",
      type: "switchSession",
    }),
  );
  const restored = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionA &&
      snapshot.ctxLabel === "Ctx 55%" &&
      snapshot.planCardCount === 1 &&
      snapshot.planStateText === "Plan: planning"
        ? snapshot
        : undefined,
    20_000,
  );
  assert.match(restored.html, /data-testid="build-plan"/u);
  assert.ok(!restored.disabledTestIds.includes("build-plan"), "expected restored Build to be enabled");
  assert.ok(
    restored.messageTexts.some((text) => /transcript ui/i.test(text)),
    "expected session A transcript to remain visible after switching back",
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("switch-restore");
  }
}

export async function assertTranscriptSwitchBackOrder(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-switch-order-new-session-a",
      type: "newSession",
    }),
  );
  const sessionA = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-switch-order-new-session-b",
      type: "newSession",
    }),
  );
  const sessionB = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId || activeSessionId === sessionA) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );
  assert.notEqual(sessionA, sessionB);

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionA },
      messageId: "webview-switch-order-prime-a",
      type: "switchSession",
    }),
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId: sessionA,
        text: "transcript ui switch back order",
      },
      messageId: "webview-switch-order-seed",
      type: "prompt",
    }),
  );

  await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionA];
      if (!session || state.activeSessionId !== sessionA || !session.busy) {
        return undefined;
      }
      const thinkingBlocks = session.timeline.filter(
        (item) => item.type === "thinking" && item.text.trim().length > 0,
      );
      const tools = session.timeline.filter((item) => item.type === "tool");
      const warnings = session.timeline.filter(
        (item) =>
          item.type === "message" &&
          item.kind === "warn" &&
          item.text === "Tomcat plan warning: rounds_exhausted",
      );
      return thinkingBlocks.length === 1 && tools.length >= 3 && warnings.length === 1
        ? state
        : undefined;
    },
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionB },
      messageId: "webview-switch-order-to-b",
      type: "switchSession",
    }),
  );

  const whileViewingB = await waitForWebviewState(
    api,
    (state) =>
      state.activeSessionId === sessionB && state.sessionViews[sessionA]?.busy ? state : undefined,
    20_000,
  );
  assert.equal(
    whileViewingB.sessionViews[sessionA]?.busy,
    true,
    "expected session A to still be busy when switching away",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionA },
      messageId: "webview-switch-order-back-to-a",
      type: "switchSession",
    }),
  );

  const restoredState = await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionA];
      if (!session || state.activeSessionId !== sessionA || session.busy) {
        return undefined;
      }
      return state;
    },
    20_000,
  );
  const restoredTimeline = restoredState.sessionViews[sessionA]?.timeline ?? [];
  const thinkingBlocks = restoredTimeline.filter(
    (
      item,
    ): item is Extract<(typeof restoredTimeline)[number], { type: "thinking" }> =>
      item.type === "thinking",
  );
  const assistantMessages = restoredTimeline.filter(
    (item) => item.type === "message" && ("kind" in item ? item.kind === "assistant" : false),
  );
  const warningMessages = restoredTimeline.filter(
    (item) =>
      item.type === "message" &&
      ("kind" in item ? item.kind === "warn" : false) &&
      item.text === "Tomcat plan warning: rounds_exhausted",
  );
  const tools = restoredTimeline.filter(
    (item): item is Extract<(typeof restoredTimeline)[number], { type: "tool" }> =>
      item.type === "tool",
  );

  assert.equal(thinkingBlocks.length, 1, "expected exactly one thinking block after switching back");
  assert.ok(
    thinkingBlocks[0]?.text.trim().length,
    "expected the restored thinking block to keep its streamed text",
  );
  assert.equal(
    assistantMessages.length,
    1,
    "expected exactly one assistant message after switching back",
  );
  assert.equal(
    warningMessages.length,
    1,
    "expected exactly one plan warning message after switching back",
  );
  assert.equal(
    tools.length,
    3,
    `expected 3 transcript tools after switching back, got ${tools.length}`,
  );
  assert.deepEqual(
    tools.map((item) => item.toolCallId),
    ["tc-transcript-read", "tc-transcript-bash", "tc-transcript-web-search"],
  );

  const firstAssistantMessage = assistantMessages[0] as
    | { assistantMessageId?: string }
    | undefined;
  const assistantMessageIds = new Set(
    [
      thinkingBlocks[0]?.assistantMessageId,
      firstAssistantMessage?.assistantMessageId,
      ...tools.map((item) => item.assistantMessageId),
    ].filter((value): value is string => typeof value === "string" && value.length > 0),
  );
  assert.equal(
    assistantMessageIds.size,
    1,
    `expected one shared assistantMessageId, got ${JSON.stringify([...assistantMessageIds])}`,
  );

  await new Promise((resolve) => setTimeout(resolve, 200));
  const restoredDom = await api.__testing.captureWebviewDom();
  assert.equal(
    restoredDom.groupFoldTitles.filter((title) => title === "Reviewed 1 file").length,
    1,
    "expected one summary title after switching back",
  );
  assert.equal(
    restoredDom.messageTexts.filter((text) => text === "Tomcat plan warning: rounds_exhausted").length,
    1,
    "expected one warning card after switching back",
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("switch-order");
  }
}

export async function assertWebviewReloadReplayFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-reload-new-session",
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "plan replay",
      },
      messageId: "webview-reload-plan-replay",
      type: "prompt",
    }),
  );
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.ctxLabel === "Ctx 62%" &&
      snapshot.planCardCount === 1 &&
      snapshot.planCardTodoCountText === "3 todos" &&
      snapshot.planCardTitleText === "Replay the plan review and verify history" &&
      snapshot.planNoticeReplayed &&
      snapshot.planStateText === "Plan: pending"
        ? snapshot
        : undefined,
    20_000,
  );

  await api.__testing.reloadWebview();
  const reloaded = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.ctxLabel === "Ctx 62%" &&
      snapshot.planCardCount === 1 &&
      snapshot.planCardTodoCountText === "3 todos" &&
      snapshot.planCardTitleText === "Replay the plan review and verify history" &&
      snapshot.planNoticeReplayed &&
      snapshot.planStateText === "Plan: pending"
        ? snapshot
        : undefined,
    20_000,
  );
  assert.equal(
    reloaded.messageTexts.filter((text) => text === "Tomcat plan review: looks good").length,
    1,
  );
  assert.equal(
    reloaded.messageTexts.filter((text) => text === "Tomcat plan verify: pass").length,
    1,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("reload-replay");
  }
}

export async function assertWebviewGiantGroupLazyLoadFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-giant-group-new-session",
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  const runPrompt = async (text: string, messageId: string) => {
    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent(
      buildWebviewIntent({
        data: { sessionId, text },
        messageId,
        type: "prompt",
      }),
    );
    await waitForEvent(api, { sessionId, type: "agent_end" });
  };

  await runPrompt("giant tool history", "webview-giant-group-showcase");
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.groupFoldTitles.some((title) => title.includes("Giant history tool group"))
        ? snapshot
        : undefined,
    20_000,
  );

  for (let index = 0; index < 12; index += 1) {
    await runPrompt(
      `hello fake tomcat follow up ${index + 1}`,
      `webview-giant-group-follow-up-${index + 1}`,
    );
  }

  await api.__testing.reloadWebview();
  const reloaded = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.messageTexts.some((text) => text.includes("hello from fake tomcat")) &&
      snapshot.toolRowCount === 0
        ? snapshot
        : undefined,
    20_000,
  );
  assert.equal(
    reloaded.toolRowCount,
    0,
    "expected no partial tool rows to render before the user expands the recovered group",
  );

  await api.__testing.sendWebviewDomAction({
    edge: "top",
    kind: "scrollToEdge",
    testId: "stream-container",
  });
  const loading = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.historyLoaderVisible &&
      snapshot.toolRowCount === 0
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(loading.historyLoaderVisible, "expected the subtle top loader while chasing the giant group");
  assert.equal(loading.toolRowCount, 0, "expected no partial tool rows while older pages are still loading");

  const restored = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      !snapshot.historyLoaderVisible &&
      snapshot.groupFoldTitles.some((title) => title.includes("Giant history tool group"))
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    restored.groupFoldTitles.some((title) => title.includes("Giant history tool group")),
    "expected the giant tool group header to appear once the head arrives",
  );

  await api.__testing.sendWebviewDomAction({
    index: -1,
    kind: "clickTestId",
    testId: "thinking-group-toggle",
  });
  const expanded = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.toolTitles.length >= 90
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    expanded.toolTitles.length >= 90,
    `expected nearly the full giant group after expansion, got ${expanded.toolTitles.length} tool rows`,
  );
}

export async function assertWebviewCrossOwnerPlanFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const participantTurn = await api.__testing.runParticipantTurn({
    prompt: "participant owner",
  });
  const sessionId = participantTurn.result?.metadata?.sessionId;
  assert.equal(typeof sessionId, "string");

  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: "webview-cross-owner-switch",
      type: "switchSession",
    }),
  );
  await waitForWebviewState(
    api,
    (candidate) => {
      const activeSessionId = candidate.activeSessionId;
      if (activeSessionId !== sessionId) {
        return undefined;
      }
      return candidate.sessionViews[sessionId!]?.conflictMessage ? candidate : undefined;
    },
  );
  const planPath = "/workspace/plans/participant-plan.plan.md";
  await api.__testing.injectServeEvent({
    sessionId: sessionId!,
    state: "planning",
    type: "plan.enter",
  });
  await api.__testing.injectServeEvent({
    path: planPath,
    planId: "participant-plan",
    sessionId: sessionId!,
    state: "planning",
    type: "plan.create",
  });

  const planning = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.hasConflict &&
      snapshot.planCardCount === 1 &&
      snapshot.planStateText === "Plan: planning"
        ? snapshot
        : undefined,
    20_000,
  );
  assert.equal(planning.planStateText, "Plan: planning");

  await api.__testing.injectServeEvent({
    path: planPath,
    planId: "participant-plan",
    sessionId: sessionId!,
    state: "executing",
    type: "plan.build",
  });
  const executing = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.hasConflict &&
      snapshot.planCardCount === 1 &&
      snapshot.planStateText === "Plan: executing"
        ? snapshot
        : undefined,
    20_000,
  );
  assert.equal(executing.planStateText, "Plan: executing");
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("cross-owner");
  }

  await api.__testing.injectServeEvent({
    path: planPath,
    planId: "participant-plan",
    sessionId: sessionId!,
    state: "chat",
    type: "plan.exit",
  });
  const settled = await waitForWebviewState(
    api,
    (candidate) => {
      const session = sessionId ? candidate.sessionViews[sessionId] : undefined;
      if (!session) {
        return undefined;
      }
      return session.planState === "chat" && session.planFile?.state === "chat"
        ? session
        : undefined;
    },
    20_000,
  );
  assert.ok(settled.planFile?.path?.endsWith("/plans/participant-plan.plan.md"));

  const exited = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.hasConflict &&
      snapshot.planCardCount === 1 &&
      snapshot.planStateText === null
        ? snapshot
        : undefined,
    20_000,
  );
  assert.equal(exited.planStateText, null);
}

function transcriptVisualArtifactPath(filename: string): string {
  const dir = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR || "/tmp";
  return path.join(dir, filename);
}

function captureTranscriptVisual(
  name:
    | "collapsed"
    | "cross-owner"
    | "expanded"
    | "file-chip"
    | "progress"
    | "reload-replay"
    | "switch-order"
    | "switch-restore"
    | "todo-expanded"
    | "tool-icons"
    | "tool-icons-bottom",
): void {
  try {
    execSync(
      `screencapture -x ${JSON.stringify(
        transcriptVisualArtifactPath(`tomcat-vsix-visual-${name}.png`),
      )}`,
    );
  } catch {
    /* screencapture unavailable in this environment */
  }
}

export async function assertTranscriptUiFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-transcript-new-session",
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId, text: "transcript ui showcase" },
      messageId: "webview-transcript-prompt",
      type: "prompt",
    }),
  );
  const busyTodo = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      !candidate.progressRow && candidate.todoWidgetVisible && candidate.planCardCount > 0
        ? candidate
        : undefined,
  );
  assert.ok(
    busyTodo.todoWidgetVisible,
    "expected the docked todo widget while the transcript flow is still busy",
  );
  assert.equal(
    busyTodo.composerPlanStatusInBarCount,
    0,
    `expected no inline plan-status chip in composer bar, got ${busyTodo.composerPlanStatusInBarCount}`,
  );
  assert.equal(
    busyTodo.composerFooterPlanStatus,
    "Plan: planning",
    `expected plan status to render in the composer footer, got ${busyTodo.composerFooterPlanStatus}`,
  );
  assert.ok(busyTodo.planFooterSameRow, "expected View Plan and Build to stay on one row");
  assert.ok(
    !busyTodo.html.includes("Tomcat is responding..."),
    "expected busy hint text to be removed from the composer",
  );
  if (process.env.TOMCAT_E2E_CAPTURE_PROGRESS === "1") {
    assert.equal(
      busyTodo.progressRow,
      false,
      "expected no inline progress row once the docked todo widget owns the busy state",
    );
    await api.__testing.focusWebview();
    captureTranscriptVisual("progress");
  }
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "todo-widget-toggle",
  });
  const expandedTodo = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.todoWidgetVisible &&
      candidate.todoWidgetExpanded &&
      candidate.todoWidgetItemCount >= 4
        ? candidate
        : undefined,
  );
  assert.equal(
    expandedTodo.todoWidgetTitle,
    "Todos (2/4)",
    `expected expanded todo widget title, got ${expandedTodo.todoWidgetTitle}`,
  );
  assert.ok(
    expandedTodo.todoWidgetItemCount >= 4,
    `expected at least 4 todo rows, got ${expandedTodo.todoWidgetItemCount}`,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("todo-expanded");
  }
  await waitForEvent(api, { type: "agent_end" });

  const collapsed = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.assistantResponseGroups >= 1 &&
      candidate.planCardCount >= 1 &&
      !candidate.progressRow &&
      !candidate.todoWidgetVisible &&
      candidate.userPromptPill &&
      candidate.assistantNoCard &&
      candidate.ellipsisAboveGroupHeader &&
      candidate.sessionTitleUpdated &&
      candidate.groupFoldTitles.some((title) => title.trim().length > 0) &&
      candidate.planCardTodoCountText === "4 todos"
        ? candidate
        : undefined,
  );
  assert.ok(
    collapsed.assistantResponseGroups >= 1,
    "expected at least one assistant response group",
  );
  assert.ok(
    collapsed.groupFoldTitles.some((title) => title.trim().length > 0),
    "expected a non-empty group fold title",
  );
  assert.ok(
    collapsed.userPromptPill,
    "expected a right-aligned user prompt pill",
  );
  assert.ok(
    collapsed.assistantNoCard,
    "expected an assistant message without a card border",
  );
  assert.ok(
    collapsed.ellipsisAboveGroupHeader,
    "expected the assistant preamble above the group header",
  );
  assert.ok(
    collapsed.planCardCount >= 1,
    "expected a visible plan card after the turn completed",
  );
  assert.equal(
    collapsed.planCardTodoCountText,
    "4 todos",
    `expected the merged plan card todo count, got ${collapsed.planCardTodoCountText}`,
  );
  assert.equal(
    collapsed.composerPlanStatusInBarCount,
    0,
    `expected plan status to stay out of the composer bar, got ${collapsed.composerPlanStatusInBarCount}`,
  );
  assert.equal(
    collapsed.composerFooterPlanStatus,
    "Plan: planning",
    `expected plan status footer text, got ${collapsed.composerFooterPlanStatus}`,
  );
  assert.ok(collapsed.planFooterSameRow, "expected the merged plan footer to stay on one row");
  assert.ok(
    !collapsed.html.includes("Tomcat is responding..."),
    "expected no responding hint after the composer cleanup",
  );
  assert.equal(collapsed.todoWidgetVisible, false, "expected no docked todo widget after the turn completes");
  assert.equal(collapsed.progressRow, false, "expected no inline progress row after the turn completes");
  assert.ok(
    collapsed.html.includes("View Plan"),
    "expected the merged plan card footer to include View Plan",
  );
  assert.ok(
    collapsed.sessionTitleUpdated,
    "expected a session.title_updated event to be observed",
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("collapsed");
  }
  await api.__testing.focusWebview();
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "thinking-group-toggle",
  });
  const expanded = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.toolRowFlat && candidate.leftGuideLine ? candidate : undefined,
  );
  assert.ok(
    expanded.toolRowFlat,
    "expected a flat tool row not wrapped in a card",
  );
  assert.ok(
    expanded.toolRowExpandable,
    "expected an expandable tool row chevron",
  );
  assert.ok(
    expanded.leftGuideLine,
    "expected the thinking-tool guide line wrapper",
  );
  assert.ok(
    expanded.toolRowCount >= 3,
    `expected at least 3 flat tool rows (read/bash/web_search), got ${expanded.toolRowCount}`,
  );
  assert.equal(
    expanded.toolCardCount,
    0,
    `expected no tool-call cards after grouping fix, got ${expanded.toolCardCount}`,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("expanded");
  }

  await api.__testing.sendWebviewDomAction({
    kind: "scrollIntoView",
    scrollBlock: "center",
    testId: "file-chip",
  });
  const fileChipReady = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.fileChipVisible &&
      typeof candidate.fileChipTopWithinStream === "number" &&
      candidate.fileChipTopWithinStream > 40 &&
      candidate.fileChipTopWithinStream < 380
        ? candidate
        : undefined,
  );
  assert.ok(fileChipReady.fileChipVisible, "expected the file chip to be visible before the close-up screenshot");
  assert.ok(
    typeof fileChipReady.fileChipTopWithinStream === "number" &&
      fileChipReady.fileChipTopWithinStream > 40 &&
      fileChipReady.fileChipTopWithinStream < 380,
    `expected file chip to be near the upper viewport, got ${fileChipReady.fileChipTopWithinStream}`,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("file-chip");
  }

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "file-chip",
  });
  const opened = await waitForWebviewDomSnapshot(
    api,
    (candidate) => (candidate.fileChipOpen ? candidate : undefined),
  );
  assert.ok(
    opened.fileChipOpen,
    "expected clicking a file chip to trigger an openFile intent",
  );

  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: "webview-tool-icons-new-session",
      type: "newSession",
    }),
  );
  const toolIconSessionId = await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: toolIconSessionId, text: "tool icon showcase" },
      messageId: "webview-tool-icons-prompt",
      type: "prompt",
    }),
  );
  await waitForEvent(api, { sessionId: toolIconSessionId, type: "agent_end" });
  const toolIconCollapsed = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.assistantResponseGroups >= 1 &&
      candidate.groupFoldTitles.some((title) => title.includes("Built-in tool icons"))
        ? candidate
        : undefined,
  );
  assert.ok(
    toolIconCollapsed.groupFoldTitles.some((title) => title.includes("Built-in tool icons")),
    "expected the tool icon showcase group title",
  );
  await api.__testing.focusWebview();
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "thinking-group-toggle",
  });
  let toolIconExpanded = await waitForWebviewDomSnapshot(
    api,
    (candidate) => (candidate.toolRowCount >= 16 ? candidate : undefined),
  );
  if (toolIconExpanded.toolRowCount < 19) {
    await api.__testing.sendWebviewDomAction({
      kind: "clickTestId",
      testId: "thinking-group-toggle",
      index: -1,
    });
    toolIconExpanded = await waitForWebviewDomSnapshot(
      api,
      (candidate) => (candidate.toolRowCount >= 19 ? candidate : undefined),
    );
  }
  assert.ok(
    toolIconExpanded.toolRowCount >= 19,
    `expected all built-in tool rows in the showcase, got ${toolIconExpanded.toolRowCount}`,
  );
  assert.ok(
    toolIconExpanded.html.includes("Loaded skill sdk"),
    "expected the showcase to include load_skill",
  );
  assert.ok(
    toolIconExpanded.html.includes("Read config llm.default_model"),
    "expected the showcase to include config_get",
  );
  assert.ok(
    toolIconExpanded.html.includes("Created plan"),
    "expected the showcase to include create_plan",
  );
  assert.ok(
    toolIconExpanded.html.includes("Asked question"),
    "expected the showcase to include ask_question",
  );
  await api.__testing.sendWebviewDomAction({
    edge: "top",
    kind: "scrollToEdge",
    testId: "stream-container",
  });
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("tool-icons");
    await api.__testing.sendWebviewDomAction({
      edge: "bottom",
      kind: "scrollToEdge",
      testId: "stream-container",
    });
    await api.__testing.focusWebview();
    captureTranscriptVisual("tool-icons-bottom");
  }
}
