import * as assert from "node:assert/strict";
import { execFileSync, execSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";

import * as vscode from "vscode";

import {
  EXTENSION_ID,
  TEST_DEFAULT_CWD_ENV,
  TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
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

async function pause(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

type MacWindowInfo = {
  bounds: {
    height: number;
    width: number;
    x: number;
    y: number;
  };
  ownerName: string;
  windowName: string;
  windowNumber: number;
};

type CaptureRegion = "editor" | "sidebar" | "window";

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

async function waitForWebviewBootstrapSettled(
  api: TomcatExtensionApi,
  timeoutMs = 40_000,
): Promise<void> {
  await waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return undefined;
      }
      const activeSessionInList = state.sessions.some(
        (session) => session.sessionId === activeSessionId,
      );
      return activeSessionInList && state.sessionViews[activeSessionId]
        ? state
        : undefined;
    },
    timeoutMs,
  );
}

async function claimActiveWebviewSession(
  api: TomcatExtensionApi,
  messageId: string,
  timeoutMs = 20_000,
): Promise<string> {
  await waitForWebviewBootstrapSettled(api);
  const sessionId = api.__testing.getWebviewState().activeSessionId;
  assert.ok(sessionId, "expected a bootstrapped active session before claiming ownership");
  api.__testing.releaseSessionOwnership(sessionId);
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId,
      type: "switchSession",
    }),
  );
  await waitForWebviewState(
    api,
    (state) =>
      state.activeSessionId === sessionId
      && state.sessionViews[sessionId]?.ownedByThisFrontend
        ? state
        : undefined,
    timeoutMs,
  );
  return sessionId;
}

async function claimDifferentWebviewSession(
  api: TomcatExtensionApi,
  currentSessionId: string,
  messageId: string,
  timeoutMs = 20_000,
): Promise<string> {
  await waitForWebviewBootstrapSettled(api);
  const candidate = api.__testing
    .getWebviewState()
    .sessions.find((session) => session.sessionId !== currentSessionId)
    ?.sessionId;
  if (candidate) {
    api.__testing.releaseSessionOwnership(candidate);
    await api.__testing.sendWebviewIntent(
      buildWebviewIntent({
        data: { sessionId: candidate },
        messageId,
        type: "switchSession",
      }),
    );
    await waitForWebviewState(
      api,
      (state) =>
        state.activeSessionId === candidate
        && state.sessionViews[candidate]?.ownedByThisFrontend
          ? state
          : undefined,
      timeoutMs,
    );
    return candidate;
  }

  const knownSessionIds = new Set(
    api.__testing.getWebviewState().sessions.map((session) => session.sessionId),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId: `${messageId}-new`,
      type: "newSession",
    }),
  );
  return waitForWebviewState(
    api,
    (state) => {
      const activeSessionId = state.activeSessionId;
      if (
        !activeSessionId
        || activeSessionId === currentSessionId
        || knownSessionIds.has(activeSessionId)
      ) {
        return undefined;
      }
      return state.sessionViews[activeSessionId]?.ownedByThisFrontend
        ? activeSessionId
        : undefined;
    },
    timeoutMs,
  );
}

async function createFreshWebviewSession(
  api: TomcatExtensionApi,
  messageId: string,
  timeoutMs = 20_000,
): Promise<string> {
  await waitForWebviewBootstrapSettled(api);
  const knownSessionIds = new Set(
    api.__testing.getWebviewState().sessions.map((session) => session.sessionId),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      messageId,
      type: "newSession",
    }),
  );
  const sessionId = await waitForWebviewState(
    api,
    (state) =>
      state.sessions.find((session) => !knownSessionIds.has(session.sessionId))
        ?.sessionId,
    timeoutMs,
  );
  api.__testing.releaseSessionOwnership(sessionId);
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId },
      messageId: `${messageId}-claim`,
      type: "switchSession",
    }),
  );
  await waitForWebviewState(
    api,
    (state) =>
      state.activeSessionId === sessionId
      && state.sessionViews[sessionId]?.ownedByThisFrontend
        ? state
        : undefined,
    timeoutMs,
  );
  return sessionId;
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

async function startPendingParticipantTurn(
  api: TomcatExtensionApi,
): Promise<{
  pending: PendingQuestionSnapshot;
  sessionId: string;
  turnPromise: ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>;
}> {
  api.__testing.clearObservedEvents();
  const turnPromise = api.__testing.runParticipantTurn({
    prompt: "answer card showcase",
  });
  const pending = await api.__testing.waitForPendingQuestion();
  if (typeof pending.sessionId !== "string") {
    throw new Error("expected pending participant turn to carry a sessionId");
  }
  return {
    pending,
    sessionId: pending.sessionId,
    turnPromise,
  };
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

export async function assertWebviewPlanModeSwitchFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const sessionId = await claimActiveWebviewSession(
    api,
    "webview-plan-mode-claim",
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { action: "enter", sessionId },
      messageId: "webview-plan-mode-enter",
      type: "setPlanMode",
    }),
  );
  await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionId];
      return session?.planState === "planning" ? session : undefined;
    },
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { action: "build", planId: "fake-plan", sessionId },
      messageId: "webview-plan-mode-build",
      type: "setPlanMode",
    }),
  );
  await waitForEvent(api, { sessionId, type: "agent_end" });
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.planStateText === "Plan: executing"
        ? snapshot
        : undefined,
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { action: "exit", sessionId },
      messageId: "webview-plan-mode-exit",
      type: "setPlanMode",
    }),
  );

  const settled = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="send-button"') &&
      !snapshot.html.includes('data-testid="stop-button"') &&
      snapshot.planStateText === null
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    !settled.timelineKinds.includes("error"),
    "executing 切回 Chat 后不应出现 error 气泡/错误消息"
  );
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
  await waitForWebviewBootstrapSettled(api);
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
  await waitForEvent(api, { type: "agent_idle" });
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.messageTexts.some((text) => /hello from fake tomcat/i.test(text))
      && candidate.html.includes('data-testid="send-button"')
      && !candidate.html.includes('data-testid="stop-button"')
        ? candidate
        : undefined,
  );
  assert.ok(
    snapshot.messageTexts.some((text) => /hello from fake tomcat/i.test(text)),
    "expected webview DOM to render the streamed assistant text",
  );
  assert.ok(
    snapshot.html.includes('data-testid="send-button"')
      && !snapshot.html.includes('data-testid="stop-button"'),
    "expected normal completion to return the webview composer to send mode",
  );
}

export async function assertWebviewInterruptFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const sessionId = await claimActiveWebviewSession(
    api,
    "webview-interrupt-claim",
  );
  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
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
  await waitForWebviewState(
    api,
    (state) => (state.sessionViews[sessionId]?.busy ? state : undefined),
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
  await waitForEvent(api, { type: "agent_idle" });

  const settled = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="send-button"') &&
      !snapshot.html.includes('data-testid="stop-button"') &&
      !snapshot.html.includes('data-testid="tool-row-running-indicator"') &&
      snapshot.messageTexts.includes("interrupt please")
        ? snapshot
        : undefined,
    20_000,
  );
  void settled;

  const otherSessionId = await claimDifferentWebviewSession(
    api,
    sessionId,
    "webview-interrupt-switch-away",
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
      snapshot.messageTexts.includes("interrupt please")
        ? snapshot
        : undefined,
    20_000,
  );
  void restored;
}

export async function assertWebviewAnswerCardFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await claimActiveWebviewSession(
    api,
    "webview-answer-card-claim",
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
  const sessionId = await claimActiveWebviewSession(
    api,
    "webview-diff-claim",
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
  const sessionA = await createFreshWebviewSession(api, "webview-new-session-a");

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { text: "thread A" },
      messageId: "webview-thread-a",
      type: "prompt",
    }),
  );
  await waitForEvent(api, { sessionId: sessionA!, type: "agent_end" });

  const sessionB = await createFreshWebviewSession(api, "webview-new-session-b");
  const stateB = api.__testing.getWebviewState();
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
  const { pending, sessionId, turnPromise } = await startPendingParticipantTurn(api);

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

  await answerPendingQuestion(pending);
  const participantTurn = await turnPromise;
  assert.equal(participantTurn.result?.errorDetails?.message, undefined);
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

  const sessionA = await createFreshWebviewSession(
    api,
    "webview-restore-new-session-a",
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

  const sessionB = await createFreshWebviewSession(
    api,
    "webview-restore-new-session-b",
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

  const sessionA = await createFreshWebviewSession(
    api,
    "webview-switch-order-new-session-a",
  );

  const sessionB = await createFreshWebviewSession(
    api,
    "webview-switch-order-new-session-b",
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
      messageId: "webview-switch-order-back-to-a-prime-history",
      type: "switchSession",
    }),
  );

  await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionA];
      if (!session || state.activeSessionId !== sessionA || !session.busy || !session.hasMoreHistory) {
        return undefined;
      }
      return state;
    },
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionA },
      messageId: "webview-switch-order-load-older",
      type: "loadOlderHistory",
    }),
  );

  await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionA];
      if (!session || state.activeSessionId !== sessionA || !session.busy) {
        return undefined;
      }
      const ghostCount = session.timeline.filter(
        (item) =>
          item.type === "message" &&
          item.kind === "user" &&
          /^ghost prompt /u.test(item.text),
      ).length;
      return ghostCount >= 5 ? state : undefined;
    },
    20_000,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionB },
      messageId: "webview-switch-order-second-to-b",
      type: "switchSession",
    }),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId: sessionA },
      messageId: "webview-switch-order-second-back-to-a",
      type: "switchSession",
    }),
  );

  const busyRestoredState = await waitForWebviewState(
    api,
    (state) => {
      const session = state.sessionViews[sessionA];
      if (!session || state.activeSessionId !== sessionA || !session.busy) {
        return undefined;
      }
      const ghostCount = session.timeline.filter(
        (item) =>
          item.type === "message" &&
          item.kind === "user" &&
          /^ghost prompt /u.test(item.text),
      ).length;
      return ghostCount >= 5 ? state : undefined;
    },
    20_000,
  );
  const busyUserMessages = (busyRestoredState.sessionViews[sessionA]?.timeline ?? []).flatMap((item) =>
    item.type === "message" && item.kind === "user" ? [item] : [],
  );
  assert.ok(busyUserMessages.length > 0, "expected user messages after switching back");
  assert.equal(
    busyUserMessages.at(-1)?.text,
    "transcript ui switch back order",
    "expected the current prompt to remain the latest user boundary while busy",
  );
  assert.ok(
    busyUserMessages.slice(-5).every((item) => !/^ghost prompt /u.test(item.text)),
    "expected old ghost prompts to stay out of the live tail after switching back",
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
  const restoredUserMessages = restoredTimeline.flatMap((item) =>
    item.type === "message" && ("kind" in item ? item.kind === "user" : false) ? [item.text] : [],
  );
  assert.equal(
    restoredUserMessages.at(-1),
    "transcript ui switch back order",
    "expected the current prompt to remain the latest user message after the turn settles",
  );
  assert.ok(
    restoredUserMessages.filter((text) => /^ghost prompt /u.test(text)).length >= 5,
    "expected older ghost prompts to remain loaded after switching back",
  );

  await new Promise((resolve) => setTimeout(resolve, 200));
  const restoredDom = await api.__testing.captureWebviewDom();
  const domCurrentPromptIndex = restoredDom.messageTexts.lastIndexOf("transcript ui switch back order");
  const domGhostFirstIndex = restoredDom.messageTexts.indexOf("ghost prompt 1");
  const domGhostLastIndex = restoredDom.messageTexts.lastIndexOf("ghost prompt 5");
  assert.ok(
    domCurrentPromptIndex >= 0,
    "expected the current prompt to remain visible after switching back",
  );
  assert.ok(
    domGhostFirstIndex >= 0 && domGhostFirstIndex < domCurrentPromptIndex,
    "expected old ghost prompts to stay ahead of the current prompt in DOM order",
  );
  assert.ok(
    domGhostLastIndex >= 0 && domGhostLastIndex < domCurrentPromptIndex,
    "expected the last ghost prompt to stay ahead of the current prompt in DOM order",
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
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-reload-new-session",
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
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-giant-group-new-session",
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
  for (const ownership of api.__testing.getOwnership()) {
    if (ownership.owner === "webview") {
      api.__testing.releaseSessionOwnership(ownership.sessionId, "webview");
    }
  }

  const { pending, sessionId, turnPromise } = await startPendingParticipantTurn(api);

  await api.__testing.reloadWebview();
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await waitForWebviewState(
    api,
    (candidate) =>
      candidate.sessions.some((session) => session.sessionId === sessionId)
        ? candidate
        : undefined,
  );
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
      return candidate.activeSessionId === sessionId ? candidate : undefined;
    },
  );
  await waitForWebviewState(
    api,
    (candidate) => {
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

  await answerPendingQuestion(pending);
  const participantTurn = await turnPromise;
  assert.equal(participantTurn.result?.errorDetails?.message, undefined);
}

export async function assertWebviewSelectionReferenceFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-selection-reference-new-session",
  );

  const workspaceDir = requireEnv(TEST_DEFAULT_CWD_ENV);
  const filePath = path.join(workspaceDir, "selection-context.ts");
  await fs.writeFile(
    filePath,
    [
      "const alpha = 1;",
      "const beta = 2;",
      "const gamma = alpha + beta;",
      "",
    ].join("\n"),
    "utf8",
  );

  const document = await vscode.workspace.openTextDocument(vscode.Uri.file(filePath));
  const editor = await vscode.window.showTextDocument(document, { preview: false });
  await vscode.workspace
    .getConfiguration("editor")
    .update("codeLens", true, vscode.ConfigurationTarget.Global);
  await pause(150);
  editor.selection = new vscode.Selection(
    new vscode.Position(1, 0),
    new vscode.Position(2, document.lineAt(2).text.length),
  );
  await pause(1_100);
  captureTranscriptVisual("selection-reference-codelens", "editor", "selection-context.ts");

  await api.__testing.executeCommand(TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND);

  const composerSnapshot = await waitForWebviewDomSnapshot(
    api,
    (snapshot) => {
      const chipCount = (snapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;
      const sendDisabled = /data-testid="send-button"[^>]*disabled/u.test(snapshot.html);
      return (
        snapshot.activeSessionId === sessionId &&
        chipCount === 1 &&
        snapshot.html.includes(`title="${filePath}:2-3"`) &&
        !sendDisabled
      )
        ? snapshot
        : undefined;
    },
    20_000,
  );
  assert.ok(
    composerSnapshot.html.includes("selection-context.ts:2-3"),
    "expected the composer chip label to include the selected file and lines",
  );
  captureTranscriptVisual("selection-reference-composer", "sidebar", "selection-context.ts");

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "send-button",
  });
  await waitForEvent(api, { sessionId, type: "agent_end" });

  await api.__testing.reloadWebview();

  type RestoredReferenceSegment = {
    lineEnd?: number | null;
    lineStart?: number | null;
    path?: string;
    type: string;
  };
  const restoredMessage = await waitForWebviewState(
    api,
    (state) => {
      const timeline = state.sessionViews[sessionId]?.timeline ?? [];
      const userMessage = [...timeline]
        .reverse()
        .find((item) => item.type === "message" && "kind" in item && item.kind === "user");
      const segments =
        userMessage && "segments" in userMessage
          ? (userMessage.segments as RestoredReferenceSegment[] | undefined)
          : undefined;
      return segments?.some(
        (segment: RestoredReferenceSegment) =>
          segment.type === "reference" &&
          segment.path === filePath &&
          segment.lineStart === 2 &&
          segment.lineEnd === 3,
      )
        ? { segments }
        : undefined;
    },
    20_000,
  );
  assert.ok(
    restoredMessage.segments?.some(
      (segment: RestoredReferenceSegment) =>
        segment.type === "reference" &&
        segment.path === filePath &&
        segment.lineStart === 2 &&
        segment.lineEnd === 3,
    ),
    "expected the reloaded transcript to preserve the selection reference segment",
  );

  const restoredSnapshot = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="history-reference-chip"') &&
      snapshot.html.includes(`title="${filePath}:2-3"`)
        ? snapshot
        : undefined,
    20_000,
  );
  assert.ok(
    restoredSnapshot.messageTexts.some((text) => text.includes("selection-context.ts:2-3")),
    "expected the restored transcript bubble to render the selection reference label",
  );
  captureTranscriptVisual("selection-reference-history", "sidebar", "selection-context.ts");
}

export async function assertWebviewFileDropReferenceFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-file-drop-reference-new-session",
  );

  const workspaceDir = requireEnv(TEST_DEFAULT_CWD_ENV);
  const filePath = path.join(workspaceDir, "drop-context.md");
  const secondFilePath = path.join(workspaceDir, "drop-context-2.md");
  await fs.writeFile(filePath, "# dropped context\n", "utf8");
  await fs.writeFile(secondFilePath, "## another dropped context\n", "utf8");
  const fileUri = vscode.Uri.file(filePath).toString();
  const secondFileUri = vscode.Uri.file(secondFilePath).toString();
  const document = await vscode.workspace.openTextDocument(vscode.Uri.file(filePath));
  await vscode.window.showTextDocument(document, { preview: false });
  await pause(300);

  const idleSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.html.includes('data-testid="composer-notice-drag"') &&
      candidate.html.includes("拖文件请按住 Shift")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    idleSnapshot.html.includes("拖文件请按住 Shift"),
    "expected the idle composer to teach the Shift drag requirement",
  );

  await api.__testing.sendWebviewDomAction({
    kind: "dragOverTestId",
    testId: "composer-surface",
  });
  const dragSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.html.includes("tc-composer__surface--drop-active") &&
      candidate.html.includes("松手加入上下文")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    dragSnapshot.html.includes("tc-composer__surface--drop-active"),
    "expected composer surface to show the drag-over highlight",
  );
  assert.ok(
    dragSnapshot.html.includes("松手加入上下文"),
    "expected the active drag hint to confirm the drop target",
  );
  captureTranscriptVisual("file-drop-reference-hover", "sidebar", "drop-context.md");
  await api.__testing.sendWebviewDomAction({
    kind: "dragLeaveTestId",
    testId: "composer-surface",
  });

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        uris: [fileUri, secondFileUri],
      },
      messageId: "webview-file-drop-reference-1",
      type: "resolveDrop",
    }),
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        uris: [fileUri],
      },
      messageId: "webview-file-drop-reference-2",
      type: "resolveDrop",
    }),
  );

  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) => {
      const chipCount = (candidate.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;
      return (
        candidate.activeSessionId === sessionId &&
        chipCount === 2 &&
        candidate.html.includes(`title="${filePath}"`) &&
        candidate.html.includes(`title="${secondFilePath}"`) &&
        candidate.html.includes("drop-context.md")
        && candidate.html.includes("drop-context-2.md")
      )
        ? candidate
        : undefined;
    },
    20_000,
  );
  assert.equal(
    (snapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length,
    2,
    "expected distinct file drops to remain while duplicate file drops dedupe away",
  );
  captureTranscriptVisual("file-drop-reference", "sidebar", "drop-context.md");
}

export async function assertWebviewPickContextFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-pick-context-new-session",
  );

  const workspaceDir = requireEnv(TEST_DEFAULT_CWD_ENV);
  const imagePath = path.join(workspaceDir, "pick-context-image.png");
  const codePath = path.join(workspaceDir, "pick-context.ts");
  const folderPath = path.join(workspaceDir, "pick-context-folder");
  await fs.writeFile(imagePath, "png-bytes", "utf8");
  await fs.writeFile(codePath, "export const pickContext = true;\n", "utf8");
  await fs.mkdir(folderPath, { recursive: true });

  const baselineSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
        ? candidate
        : undefined,
    20_000,
  );
  const baselineAttachmentCount =
    (baselineSnapshot.html.match(/data-testid="attachment-chip"/gu) ?? []).length;
  const baselineReferenceCount =
    (baselineSnapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;

  api.__testing.setOpenDialogHandler(() => [
    vscode.Uri.file(imagePath),
    vscode.Uri.file(codePath),
    vscode.Uri.file(folderPath),
  ]);

  try {
    await api.__testing.sendWebviewDomAction({
      kind: "clickTestId",
      testId: "attachment-add",
    });

    const snapshot = await waitForWebviewDomSnapshot(
      api,
      (candidate) => {
        const attachmentCount = (candidate.html.match(/data-testid="attachment-chip"/gu) ?? []).length;
        const referenceCount = (candidate.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;
        return (
          candidate.activeSessionId === sessionId &&
          attachmentCount === baselineAttachmentCount + 1 &&
          referenceCount === baselineReferenceCount + 2 &&
          candidate.html.includes("pick-context-image.png") &&
          candidate.html.includes("pick-context.ts") &&
          candidate.html.includes("pick-context-folder/")
        )
          ? candidate
          : undefined;
      },
      20_000,
    );

    assert.equal(
      (snapshot.html.match(/data-testid="attachment-chip"/gu) ?? []).length,
      baselineAttachmentCount + 1,
      "expected the picker to add exactly one pending attachment",
    );
    assert.equal(
      (snapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length,
      baselineReferenceCount + 2,
      "expected the picker to add two context reference chips",
    );

    const settled = await waitForWebviewState(
      api,
      (state) => {
        const view = state.sessionViews[sessionId];
        if (!view || view.pendingAttachments.length !== 1) {
          return undefined;
        }
        return {
          attachments: view.pendingAttachments,
        };
      },
      20_000,
    );

    assert.equal(settled.attachments[0]?.label, "pick-context-image.png");
    assert.equal(settled.attachments[0]?.kind, "image");
  } finally {
    api.__testing.setOpenDialogHandler(undefined);
  }
}

function transcriptVisualArtifactPath(filename: string): string {
  const dir = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR || "/tmp";
  return path.join(dir, filename);
}

function locateMacosWindowScriptPath(): string {
  return path.resolve(__dirname, "../../../../scripts/find-macos-window.swift");
}

function resolveCaptureRect(
  bounds: MacWindowInfo["bounds"],
  region: CaptureRegion,
): { height: number; width: number; x: number; y: number } {
  if (region === "window") {
    return bounds;
  }

  const topInset = region === "editor"
    ? Math.min(52, Math.max(18, Math.round(bounds.height * 0.03)))
    : Math.min(86, Math.max(62, Math.round(bounds.height * 0.09)));
  const bottomInset = 28;
  const usableHeight = Math.max(240, bounds.height - topInset - bottomInset);

  if (region === "sidebar") {
    const width = Math.min(440, Math.max(360, Math.round(bounds.width * 0.36)));
    return {
      height: usableHeight,
      width,
      x: bounds.x + bounds.width - width - 16,
      y: bounds.y + topInset,
    };
  }

  const width = Math.min(760, Math.max(560, Math.round(bounds.width * 0.48)));
  return {
    height: Math.min(700, usableHeight),
    width,
    x: bounds.x + Math.max(80, Math.round(bounds.width * 0.28)),
    y: bounds.y + topInset,
  };
}

function tryResolveVsCodeWindow(appName: string): MacWindowInfo | null {
  return tryResolveVsCodeWindowWithTitle(appName);
}

function tryResolveVsCodeWindowWithTitle(
  appName: string,
  titleHint?: string,
): MacWindowInfo | null {
  try {
    const args = [locateMacosWindowScriptPath(), appName];
    if (titleHint && titleHint.trim().length > 0) {
      args.push("--title", titleHint);
    }
    const raw = execFileSync(
      "swift",
      args,
      {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      },
    ).trim();
    return raw ? JSON.parse(raw) as MacWindowInfo : null;
  } catch {
    return null;
  }
}

function captureTranscriptVisual(
  name:
    | "collapsed"
    | "cross-owner"
    | "expanded"
    | "file-drop-reference"
    | "file-drop-reference-hover"
    | "file-chip"
    | "progress"
    | "reload-replay"
    | "selection-reference-codelens"
    | "selection-reference-composer"
    | "selection-reference-history"
    | "switch-order"
    | "switch-restore"
    | "todo-expanded"
    | "tool-icons"
    | "tool-icons-bottom",
  region: CaptureRegion = "window",
  titleHint?: string,
): void {
  try {
    const appName = vscode.env.appName || "Visual Studio Code";
    execFileSync("open", ["-a", appName], {
      stdio: "ignore",
      timeout: 2_000,
    });
    execSync("sleep 0.35");
    const targetPath = transcriptVisualArtifactPath(`tomcat-vsix-visual-${name}.png`);
    const windowInfo = tryResolveVsCodeWindowWithTitle(appName, titleHint) ?? tryResolveVsCodeWindow(appName);
    if (windowInfo) {
      const rect = resolveCaptureRect(windowInfo.bounds, region);
      execFileSync(
        "screencapture",
        [
          "-x",
          "-R",
          `${Math.round(rect.x)},${Math.round(rect.y)},${Math.round(rect.width)},${Math.round(rect.height)}`,
          targetPath,
        ],
        { stdio: "ignore" },
      );
      return;
    }
    execFileSync("screencapture", ["-x", targetPath], {
      stdio: "ignore",
    });
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
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-transcript-new-session",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { sessionId, text: "transcript ui showcase" },
      messageId: "webview-transcript-prompt",
      type: "prompt",
    }),
  );
  const requireBusyProgress = process.env.TOMCAT_E2E_CAPTURE_PROGRESS === "1";
  const busyStageTimeoutMs = requireBusyProgress ? 15_000 : 3_000;
  const collapsedPredicate = (candidate: Awaited<ReturnType<TomcatExtensionApi["__testing"]["captureWebviewDom"]>>) =>
    candidate.assistantResponseGroups >= 1 &&
    candidate.planCardCount >= 1 &&
    !candidate.progressRow &&
    !candidate.todoWidgetVisible &&
    candidate.userPromptPill &&
    candidate.assistantNoCard &&
    candidate.ellipsisAboveGroupHeader &&
    candidate.sessionTitleUpdated &&
    candidate.groupFoldTitles.some((title) => title.trim().length > 0) &&
    candidate.planCardTodoCountText === "4 todos" &&
    candidate.composerFooterPlanStatus === "Plan: planning"
      ? candidate
      : undefined;
  let collapsedFromBusyFallback:
    | Awaited<ReturnType<TomcatExtensionApi["__testing"]["captureWebviewDom"]>>
    | null = null;
  try {
    const busyTodo = await waitForWebviewDomSnapshot(
      api,
      (candidate) =>
        !candidate.progressRow &&
        candidate.todoWidgetVisible &&
        candidate.planCardCount > 0 &&
        candidate.composerFooterPlanStatus === "Plan: planning"
          ? candidate
          : undefined,
      busyStageTimeoutMs,
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
    if (requireBusyProgress) {
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
      busyStageTimeoutMs,
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
  } catch (error) {
    if (requireBusyProgress) {
      throw error;
    }
    const snapshot = await api.__testing.captureWebviewDom();
    const collapsed = collapsedPredicate(snapshot);
    if (!collapsed) {
      throw error;
    }
    collapsedFromBusyFallback = collapsed;
  }
  await waitForEvent(api, { type: "agent_end" });

  const collapsed =
    collapsedFromBusyFallback
    ?? await waitForWebviewDomSnapshot(api, collapsedPredicate);
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
  const toolIconSessionId = await createFreshWebviewSession(
    api,
    "webview-tool-icons-new-session",
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
