import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";

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
  while (Date.now() - startedAt < timeoutMs) {
    const snapshot = await api.__testing.captureWebviewDom();
    const result = predicate(snapshot);
    if (result !== undefined) {
      return result;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("Timed out waiting for webview DOM to match the expected condition");
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

  const snapshot = await api.__testing.captureWebviewDom();
  assert.ok(
    snapshot.sessionTabs.length >= 2,
    "expected the webview DOM to render multiple session tabs",
  );
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
