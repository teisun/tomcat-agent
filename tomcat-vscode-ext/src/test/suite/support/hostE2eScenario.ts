import * as assert from "node:assert/strict";
import { execFileSync, execSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";

import * as vscode from "vscode";

import {
  EXTENSION_ID,
  TEST_DEFAULT_CWD_ENV,
  TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
  TOMCAT_PLAN_ADD_SELECTION_TO_CHAT_COMMAND,
} from "../../../constants";
import { resolveUriToFileReference } from "../../../ui/webview/contextReferences";
import type {
  ObservedEventFilter,
  TomcatExtensionApi,
  WebviewIntent,
} from "../../../extension";
import type { SettingsIntent } from "../../../shared/settingsProtocol";

let dummyLanguageModelRegistration: vscode.Disposable | undefined;
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

async function waitFor(
  predicate: () => boolean,
  timeoutMs = 20_000,
  errorMessage = "Timed out waiting for condition",
): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) {
      return;
    }
    await pause(100);
  }
  throw new Error(errorMessage);
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

async function waitForPreparedChange(
  api: TomcatExtensionApi,
  toolCallId: string,
  predicate?: (
    change: NonNullable<ReturnType<TomcatExtensionApi["__testing"]["getPreparedChange"]>>,
  ) => boolean,
  timeoutMs = 15_000,
): Promise<NonNullable<ReturnType<TomcatExtensionApi["__testing"]["getPreparedChange"]>>> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const change = api.__testing.getPreparedChange(toolCallId);
    if (change && (!predicate || predicate(change))) {
      return change;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for prepared change ${toolCallId}`);
}

async function waitForSettingsPanelState<T>(
  api: TomcatExtensionApi,
  predicate: (
    state: ReturnType<TomcatExtensionApi["__testing"]["getSettingsPanelState"]>,
  ) => T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const state = api.__testing.getSettingsPanelState();
    const result = predicate(state);
    if (result !== undefined) {
      return result;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("Timed out waiting for settings panel state to match the expected condition");
}

async function waitForVisiblePreparedDiffEditors(
  toolCallId: string,
  timeoutMs = 15_000,
): Promise<readonly vscode.TextEditor[]> {
  const startedAt = Date.now();
  const encodedToolCallId = encodeURIComponent(toolCallId);
  while (Date.now() - startedAt < timeoutMs) {
    const editors = vscode.window.visibleTextEditors.filter(
      (editor) =>
        editor.document.uri.scheme === "tomcat-diff"
        && editor.document.uri.path.split("/").filter(Boolean)[0] === encodedToolCallId,
    );
    if (editors.length >= 2) {
      return editors;
    }
    await pause(100);
  }
  throw new Error(`Timed out waiting for visible diff editors for ${toolCallId}`);
}

async function waitForWebviewBootstrapSettled(
  api: TomcatExtensionApi,
  timeoutMs = 40_000,
): Promise<void> {
  await waitForWebviewState(
    api,
    (state) => {
      if (!state.ready) {
        return undefined;
      }
      const activeSessionId = state.activeSessionId;
      if (!activeSessionId) {
        return state.sessions.length === 0 ? state : undefined;
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
  const bootstrapState = api.__testing.getWebviewState();
  if (!bootstrapState.activeSessionId && bootstrapState.sessions.length === 0) {
    return createFreshWebviewSession(api, `${messageId}-bootstrap`, timeoutMs);
  }
  const sessionId = bootstrapState.activeSessionId;
  assert.ok(sessionId, "expected a bootstrapped active session before switching sessions");
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
        planCardTopWithinStream: lastSnapshot.planCardTopWithinStream,
        planNoticeReplayed: lastSnapshot.planNoticeReplayed,
        planStateText: lastSnapshot.planStateText,
        progressRow: lastSnapshot.progressRow,
        loadingShimmerCount: lastSnapshot.loadingShimmerCount,
        planTodos: lastSnapshot.planTodos,
        standaloneThinkingTitles: lastSnapshot.standaloneThinkingTitles,
        todoWidgetVisible: lastSnapshot.todoWidgetVisible,
        todoWidgetExpanded: lastSnapshot.todoWidgetExpanded,
        todoWidgetItemCount: lastSnapshot.todoWidgetItemCount,
        todoWidgetTitle: lastSnapshot.todoWidgetTitle,
        toolRowFlat: lastSnapshot.toolRowFlat,
        toolRowExpandable: lastSnapshot.toolRowExpandable,
        actionToolRowCount: lastSnapshot.actionToolRowCount,
        editDiffBadgeCount: lastSnapshot.editDiffBadgeCount,
        commandBlockCount: lastSnapshot.commandBlockCount,
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

async function waitForContextSearchIntent(
  api: TomcatExtensionApi,
  predicate: (
    intent: Extract<WebviewIntent, { type: "searchContext" }>,
  ) => boolean,
  timeoutMs = 15_000,
): Promise<Extract<WebviewIntent, { type: "searchContext" }>> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const intent = api.__testing.getLastContextSearchIntent();
    if (intent && predicate(intent)) {
      return intent;
    }
    await pause(100);
  }
  throw new Error("Timed out waiting for context search intent");
}

async function setComposerInputValue(
  api: TomcatExtensionApi,
  value: string,
): Promise<void> {
  await api.__testing.sendWebviewDomAction({
    kind: "setInputValue",
    testId: "composer-input",
    value,
  });
}

async function clearComposerDraft(
  api: TomcatExtensionApi,
  sessionId: string,
): Promise<void> {
  await api.__testing.sendWebviewDomAction({
    kind: "setInputValue",
    value: "",
  });
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      !snapshot.html.includes('data-testid="composer-reference-chip"')
        ? snapshot
        : undefined,
    10_000,
  );
}

function buildWebviewIntent(
  intent: Exclude<WebviewIntent, { type: "__test.dom_snapshot" }>,
): Exclude<WebviewIntent, { type: "__test.dom_snapshot" }> {
  return intent;
}

function stripTerminalNewline(value: string): string {
  return value.replace(/\r?\n$/u, "");
}

function assertPreparedChangeMatches(
  change: NonNullable<ReturnType<TomcatExtensionApi["__testing"]["getPreparedChange"]>>,
  displayPath: string,
  expectedBefore: string,
  expectedAfter: string,
): void {
  assert.equal(change.displayPath, displayPath);
  assert.notEqual(change.originalContent.length, 0, "expected reconstructed original content");
  assert.notEqual(change.proposedContent.length, 0, "expected reconstructed proposed content");
  assert.equal(stripTerminalNewline(change.originalContent), expectedBefore);
  assert.equal(stripTerminalNewline(change.proposedContent), expectedAfter);
}

function buildSettingsIntent(intent: SettingsIntent): SettingsIntent {
  return intent;
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

/**
 * Regression guard for the settings key-slot / API-key alignment fix. Measure
 * the visible bordered boxes the user actually sees: the key-slot combobox
 * wrapper and the API-key input. The refresh button must keep the label rows
 * aligned, and the API-key input must share the combobox's 38px control height.
 * Real layout geometry only exists in the host webview, so this assertion runs
 * here rather than in jsdom unit tests. Relay mode is used because it always
 * renders the shared key fields row (`showSharedFormFields`).
 */
async function assertSettingsKeyFieldsAligned(
  api: TomcatExtensionApi,
): Promise<void> {
  const deadline = Date.now() + 15_000;
  let dom:
    | Awaited<ReturnType<TomcatExtensionApi["__testing"]["captureSettingsDom"]>>
    | undefined;
  while (Date.now() < deadline) {
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "settings-add-model",
    });
    await pause(150);
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "settings-mode-relay",
    });
    await pause(150);
    try {
      dom = await api.__testing.captureSettingsDom();
    } catch {
      // The settings webview may still be mounting; retry until the deadline.
      await pause(150);
      continue;
    }
    if (dom.rects?.keySlotBox && dom.rects?.apiKeyInput) {
      break;
    }
  }

  const keySlot = dom?.rects?.keySlotBox;
  const apiKey = dom?.rects?.apiKeyInput;
  assert.ok(
    keySlot,
    "expected the settings DOM snapshot to expose the key-slot box rect",
  );
  assert.ok(
    apiKey,
    "expected the settings DOM snapshot to expose the API-key input rect",
  );

  const topDelta = Math.abs(keySlot.top - apiKey.top);
  assert.ok(
    topDelta <= 1,
    `expected key-slot and API-key inputs to align within 1px, got ${topDelta.toFixed(2)}px `
      + `(keySlot.top=${keySlot.top.toFixed(2)}, apiKey.top=${apiKey.top.toFixed(2)})`,
  );
  const heightDelta = Math.abs(keySlot.height - apiKey.height);
  assert.ok(
    heightDelta <= 1,
    `expected key-slot and API-key controls to share height within 1px, got ${heightDelta.toFixed(2)}px `
      + `(keySlot.height=${keySlot.height.toFixed(2)}, apiKey.height=${apiKey.height.toFixed(2)})`,
  );

  await api.__testing.sendSettingsDomAction({
    kind: "clickTestId",
    testId: "settings-close-model-form",
  });
  await pause(150);
}

export async function assertWebviewAddModelsFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await waitForWebviewBootstrapSettled(api);

  const relayProvider = `host-e2e-relay-${Date.now().toString(36)}`;
  const modelName = "gpt-5.4";
  const modelId = `${relayProvider}/${modelName}`;
  const keyEnv = "TOMCAT_ADD_MODELS_E2E_API_KEY";
  const relayBaseUrl = `https://${relayProvider}.example.test/v1`;
  const sessionId = await claimActiveWebviewSession(
    api,
    "webview-add-models-claim",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        route: "models",
      },
      messageId: "webview-open-model-settings",
      type: "openModelSettings",
    }),
  );

  const settingsSnapshot = await waitForSettingsPanelState(
    api,
    (snapshot) =>
      snapshot.visible
      && snapshot.route === "models"
      && snapshot.state.ready
      && snapshot.webviewReady
        ? snapshot
        : undefined,
    20_000,
  );
  const builtinModels = settingsSnapshot.state.models.filter(
    (model) => model.source === "builtin",
  );
  assert.ok(
    builtinModels.length > 0,
    "expected the settings panel to expose builtin models so official presets are available",
  );
  assert.strictEqual(
    settingsSnapshot.state.serverVersion,
    "0.1.17",
    "expected the settings panel state to carry the fake serve version",
  );
  assert.strictEqual(
    settingsSnapshot.state.expectedCliVersion,
    settingsSnapshot.state.serverVersion,
    "expected the extension to consider the connected fake serve version current",
  );
  const settingsDom = await api.__testing.captureSettingsDom();
  assert.ok(
    settingsDom.html.includes(`Extension v${settingsSnapshot.state.extensionVersion}`),
    "expected the settings footer to render the extension version",
  );
  assert.ok(
    settingsDom.html.includes(`Serve v${settingsSnapshot.state.serverVersion}`),
    "expected the settings footer to render the serve version",
  );

  await assertSettingsKeyFieldsAligned(api);

  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    const openAddModelForm = async (): Promise<void> => {
      const startedAt = Date.now();
      while (Date.now() - startedAt < 10_000) {
        await api.__testing.sendSettingsDomAction({
          kind: "clickTestId",
          testId: "settings-add-model",
        });
        await pause(200);
        const dom = await api.__testing.captureSettingsDom();
        if (dom.html.includes('data-testid="settings-model-form"')) {
          return;
        }
      }
      throw new Error("Timed out waiting for the add-model form to open in the settings panel");
    };
    await openAddModelForm();
    await pause(400);
    captureTranscriptVisual("settings-alignment", "window", "Tomcat Settings");
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "settings-close-model-form",
    });
    await pause(250);
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
  }

  await api.__testing.sendSettingsIntent(
    buildSettingsIntent({
      data: {
        model: {
          api: "openai",
          apiKeyEnv: keyEnv,
          baseUrl: relayBaseUrl,
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: false,
            webSearch: false,
          },
          contextWindow: 16_384,
          id: modelId,
          modelName,
          provider: relayProvider,
          thinkingFormat: null,
        },
      },
      messageId: "settings-upsert-model",
      type: "upsertModel",
    }),
  );

  await api.__testing.sendSettingsIntent(
    buildSettingsIntent({
      data: {
        envName: keyEnv,
        value: "host-e2e-key",
      },
      messageId: "settings-set-provider-key",
      type: "setProviderKey",
    }),
  );

  const materializedModel = await waitForSettingsPanelState(
    api,
    (snapshot) => {
      const model = snapshot.state.models.find((candidate) => candidate.id === modelId);
      return model?.keyPresent &&
          model.thinkingFormat === "openai" &&
          model.supportedReasoningLevels?.includes("xhigh")
        ? model
        : undefined;
    },
    20_000,
  );
  assert.ok(
    materializedModel.supportedReasoningLevels?.includes("xhigh"),
    "expected the materialized relay model to expose xhigh in supportedReasoningLevels",
  );

  await api.__testing.sendSettingsIntent(
    buildSettingsIntent({
      data: {
        model: {
          api: "openai",
          apiKeyEnv: keyEnv,
          baseUrl: relayBaseUrl,
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: false,
            webSearch: false,
          },
          contextWindow: 16_384,
          id: modelId,
          modelName,
          provider: relayProvider,
          thinkingFormat: "anthropic",
        },
      },
      messageId: "settings-upsert-model-warning",
      type: "upsertModel",
    }),
  );

  const warningState = await waitForSettingsPanelState(
    api,
    (snapshot) => {
      const model = snapshot.state.models.find((candidate) => candidate.id === modelId);
      return model?.thinkingFormat === "anthropic" &&
          snapshot.state.warnings?.some((warning) => warning.includes("reasoning effort"))
        ? snapshot
        : undefined;
    },
    20_000,
  );
  assert.ok(
    warningState.state.warnings?.some((warning) => warning.includes("reasoning effort")),
    "expected mismatched anthropic thinking format to surface a reasoning effort warning",
  );

  await api.__testing.sendSettingsIntent(
    buildSettingsIntent({
      data: {
        model: {
          api: "openai",
          apiKeyEnv: keyEnv,
          baseUrl: relayBaseUrl,
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: false,
            webSearch: false,
          },
          contextWindow: 16_384,
          id: modelId,
          modelName,
          provider: relayProvider,
          thinkingFormat: null,
        },
      },
      messageId: "settings-upsert-model-restored",
      type: "upsertModel",
    }),
  );

  await waitForSettingsPanelState(
    api,
    (snapshot) => {
      const model = snapshot.state.models.find((candidate) => candidate.id === modelId);
      return model?.thinkingFormat === "openai" &&
          (!snapshot.state.warnings || snapshot.state.warnings.length === 0)
        ? model
        : undefined;
    },
    20_000,
  );

  await waitForWebviewState(
    api,
    (state) =>
      state.availableModels.includes(modelId) &&
        state.availableModelReasoningLevels?.[modelId]?.includes("xhigh")
        ? state
        : undefined,
    20_000,
  );

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "model-select",
  });
  const dropdown = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.html.includes('data-testid="model-dropdown"') &&
      snapshot.html.includes(modelId) &&
      snapshot.modelDropdownHeight > 0
        ? snapshot
        : undefined,
    10_000,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("model-dropdown-open", "window", "Extension Development Host");
  }
  const modelSelectTop = dropdown.composerControlMetrics["model-select"]?.top ?? null;
  assert.ok(
    dropdown.modelDropdownFullyVisible,
    `expected the model dropdown to be fully visible, got top=${dropdown.modelDropdownTop}, bottom=${dropdown.modelDropdownBottom}, left=${dropdown.modelDropdownLeft}, right=${dropdown.modelDropdownRight}, height=${dropdown.modelDropdownHeight}, triggerTop=${modelSelectTop}`,
  );
  assert.ok(
    dropdown.modelDropdownTop !== null && dropdown.modelDropdownTop >= 0,
    `expected the model dropdown to stay inside the viewport, got top=${dropdown.modelDropdownTop}`,
  );
  assert.ok(
    dropdown.modelDropdownBottom !== null &&
      modelSelectTop !== null &&
      dropdown.modelDropdownBottom <= modelSelectTop,
    `expected the model dropdown to open upward above the trigger, got dropdownBottom=${dropdown.modelDropdownBottom}, triggerTop=${modelSelectTop}`,
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        modelId,
        sessionId,
      },
      messageId: "webview-set-added-model",
      type: "setModel",
    }),
  );

  await waitForSessionState(
    api,
    (state) =>
      state.sessionId === sessionId && state.model === modelId ? state : undefined,
    20_000,
  );
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "model-select",
  });
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.html.includes('data-testid="model-dropdown"')
        ? undefined
        : snapshot,
    10_000,
  );

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "thinking-level-select",
  });
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.html.includes('data-testid="thinking-level-dropdown"') &&
        snapshot.html.includes("Xhigh")
        ? snapshot
        : undefined,
    10_000,
  );
  await api.__testing.sendWebviewDomAction({
    index: 3,
    kind: "clickTestId",
    testId: "thinking-level-option",
  });
  await waitForSessionState(
    api,
    (state) =>
      state.sessionId === sessionId &&
        state.model === modelId &&
        state.thinkingLevel === "xhigh"
        ? state
        : undefined,
    20_000,
  );
  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.html.includes('data-testid="thinking-level-dropdown"')
        ? undefined
        : snapshot,
    10_000,
  );

  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "reasoning effort probe",
      },
      messageId: "webview-prompt-added-model",
      type: "prompt",
    }),
  );
  await waitForEvent(api, {
    sessionId,
    textIncludes: "reasoning effort: xhigh",
    type: "message_update",
  });
  await waitForEvent(api, {
    sessionId,
    type: "agent_idle",
  });
}

export async function assertWebviewMaxReasoningAndLoadingGapFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const sessionId = await createFreshWebviewSession(api, "webview-max-loading-gap-session");

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        modelId: "claude-4.6-sonnet",
        sessionId,
      },
      messageId: "webview-set-claude-model",
      type: "setModel",
    }),
  );
  await waitForSessionState(
    api,
    (state) =>
      state.sessionId === sessionId && state.model === "claude-4.6-sonnet" ? state : undefined,
    20_000,
  );

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "thinking-level-select",
  });
  const dropdown = await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="thinking-level-dropdown"') &&
      snapshot.html.includes("Max")
        ? snapshot
        : undefined,
    10_000,
  );
  assert.ok(
    dropdown.html.includes("Max"),
    `expected the reasoning menu to expose Max, got html=${dropdown.html.slice(0, 400)}`,
  );
  await api.__testing.sendWebviewDomAction({
    index: 3,
    kind: "clickTestId",
    testId: "thinking-level-option",
  });
  await waitForSessionState(
    api,
    (state) =>
      state.sessionId === sessionId &&
      state.model === "claude-4.6-sonnet" &&
      state.thinkingLevel === "max"
        ? state
        : undefined,
    20_000,
  );

  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "reasoning effort probe",
      },
      messageId: "webview-max-probe",
      type: "prompt",
    }),
  );
  await waitForEvent(api, {
    sessionId,
    textIncludes: "reasoning effort: max",
    type: "message_update",
  });
  await waitForEvent(api, {
    sessionId,
    type: "agent_idle",
  });

  api.__testing.clearObservedEvents();
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "loading gap showcase",
      },
      messageId: "webview-loading-gap",
      type: "prompt",
    }),
  );

  const progressSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.html.includes('data-testid="progress-row-dots"') &&
      !candidate.html.includes('data-testid="progress-row-label"') &&
      candidate.html.includes('data-testid="stop-button"')
        ? candidate
        : undefined,
    15_000,
  );
  assert.equal(progressSnapshot.progressRow, true, "expected a pre-stream inline progress row");
  assert.ok(
    progressSnapshot.html.includes('data-testid="progress-row-dots"'),
    "expected the pre-stream gap to render a dots-only progress row",
  );
  assert.ok(
    !progressSnapshot.html.includes('data-testid="progress-row-label"'),
    "expected the pre-stream gap to drop the visible Thinking label",
  );
  assert.ok(
    !progressSnapshot.html.includes("tc-codicon-spin") &&
      !progressSnapshot.html.includes("codicon-loading"),
    "expected the pre-stream gap to avoid loading spinner icons",
  );

  const thinkingSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.loadingShimmerCount > 0 &&
      candidate.standaloneThinkingTitles.includes("Thinking") &&
      !candidate.standaloneThinkingTitles.includes("Tomcat · Thinking")
        ? candidate
        : undefined,
    15_000,
  );
  assert.ok(
    thinkingSnapshot.standaloneThinkingTitles.includes("Thinking"),
    `expected the standalone thinking header to read "Thinking", got ${JSON.stringify(thinkingSnapshot.standaloneThinkingTitles)}`,
  );
  assert.ok(
    !thinkingSnapshot.standaloneThinkingTitles.includes("Tomcat · Thinking"),
    `expected the product prefix to stay removed, got ${JSON.stringify(thinkingSnapshot.standaloneThinkingTitles)}`,
  );
  assert.ok(
    thinkingSnapshot.html.includes("codicon-lightbulb") &&
      !thinkingSnapshot.html.includes("tc-codicon-spin") &&
      !thinkingSnapshot.html.includes("codicon-loading"),
    "expected standalone thinking to use a static lightbulb instead of a spinner",
  );

  await waitForEvent(api, {
    sessionId,
    textIncludes: "loading gap complete",
    type: "message_update",
  });
  await waitForEvent(api, {
    sessionId,
    type: "agent_idle",
  });
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
  const sessionId = snapshot.activeSessionId;
  assert.ok(sessionId, "expected an active session after the streaming flow completes");

  await api.__testing.injectServeEvent({
    args: { command: "npm test -- --watch=false" },
    sessionId,
    toolCallId: "streaming-bash-error-1",
    toolName: "bash",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    isError: true,
    result: "npm ERR! missing script: test",
    sessionId,
    toolCallId: "streaming-bash-error-1",
    toolName: "bash",
    type: "tool_execution_end",
  });
  const commandSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.commandBlockCount >= 1 &&
      candidate.html.includes("npm test -- --watch=false") &&
      candidate.html.includes("npm ERR! missing script: test")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    commandSnapshot.commandBlockCount >= 1,
    `expected an errored bash tool to auto-expand into a terminal block, got ${commandSnapshot.commandBlockCount}`,
  );
  // The full command now lives in the terminal body as a `$ …` prompt line; the
  // header shows a short purpose ("Ran" placeholder) + command-name tags.
  assert.ok(
    commandSnapshot.expandedToolTitles.some((title) => title.startsWith("Ran")),
    `expected the errored bash row to auto-expand with a "Ran" header, got ${JSON.stringify(commandSnapshot.expandedToolTitles)}`,
  );
  assert.ok(
    commandSnapshot.html.includes("npm test -- --watch=false"),
    "expected the full command to render in the bash terminal body",
  );

  // A successful bash whose title is asynchronously upgraded by utility-flash via
  // a `tool.summary_updated` event; assert the header adopts the purpose phrase
  // while the full command stays in the terminal body.
  await api.__testing.injectServeEvent({
    args: { command: "git status && echo done" },
    sessionId,
    toolCallId: "streaming-bash-summary-1",
    toolName: "bash",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    isError: false,
    result: "On branch main\ndone",
    sessionId,
    toolCallId: "streaming-bash-summary-1",
    toolName: "bash",
    type: "tool_execution_end",
  });
  await api.__testing.injectServeEvent({
    sessionId,
    summaryTitle: "Check git status",
    toolCallId: "streaming-bash-summary-1",
    type: "tool.summary_updated",
  });
  const summarySnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.toolTitles.some((title) => title.includes("Check git status")) &&
      candidate.html.includes("git status &amp;&amp; echo done")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    summarySnapshot.toolTitles.some(
      (title) => title.includes("Check git status") && title.includes("git"),
    ),
    `expected the bash header to show the utility purpose + command tags, got ${JSON.stringify(summarySnapshot.toolTitles)}`,
  );
  assert.ok(
    !summarySnapshot.toolTitles.some((title) => title.includes("git status && echo done")),
    "expected the full command to stay out of the bash header",
  );

  await api.__testing.injectServeEvent({
    sessionId,
    type: "agent_start",
  });
  await api.__testing.injectServeEvent({
    assistantMessageId: "streaming-context-group-1",
    assistantMessageEvent: {
      delta: "Inspecting the README before wrapping up.",
      kind: "thinking_delta",
    },
    message: {},
    sessionId,
    type: "message_update",
  });
  await api.__testing.injectServeEvent({
    args: { path: "README.md" },
    sessionId,
    toolCallId: "streaming-context-tool-1",
    toolName: "read",
    type: "tool_execution_start",
  });
  const runningGroupSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.html.includes("tc-thinking__title--shimmer") &&
      candidate.groupFoldTitles.some((title) => title.includes("README.md")) &&
      !candidate.html.includes("tc-codicon-spin") &&
      !candidate.html.includes("codicon-loading")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    runningGroupSnapshot.progressRow,
    "expected the shared progress row to stay mounted while the live context group is running",
  );
  assert.ok(
    runningGroupSnapshot.html.includes("tc-thinking__title--shimmer"),
    "expected the live context group header to shimmer while its tool is running",
  );
  assert.ok(
    runningGroupSnapshot.groupFoldTitles.some((title) => title.includes("README.md")),
    `expected the live context group title to reflect the README read, got ${JSON.stringify(runningGroupSnapshot.groupFoldTitles)}`,
  );

  await api.__testing.injectServeEvent({
    display: { file: "README.md", kind: "file" },
    isError: false,
    result: "# readme\n",
    sessionId,
    toolCallId: "streaming-context-tool-1",
    toolName: "read",
    type: "tool_execution_end",
  });
  const settledBeforeUpgrade = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.html.includes('data-testid="progress-row-dots"') &&
      !candidate.html.includes("tc-thinking__title--shimmer") &&
      candidate.groupFoldTitles.some((title) => title.includes("Read file README.md")) &&
      !candidate.html.includes("tc-codicon-spin") &&
      !candidate.html.includes("codicon-loading")
        ? candidate
        : undefined,
    20_000,
  );
  assert.equal(
    settledBeforeUpgrade.html.includes("tc-thinking__title--shimmer"),
    false,
    "expected the group header shimmer to stop as soon as the tool completed",
  );
  assert.equal(
    settledBeforeUpgrade.progressRow,
    true,
    "expected the completed-tool gap to fall back to the shared progress row",
  );

  await api.__testing.injectServeEvent({
    assistantMessageId: "streaming-context-group-1",
    message: {},
    sessionId,
    summaryTitle: "Used 1 tool",
    toolCallIds: ["streaming-context-tool-1"],
    toolResults: [{}],
    turnIndex: 1,
    type: "turn_end",
  });
  const fallbackSummarySnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.groupFoldTitles.some((title) => title.includes("Used 1 tool")) &&
      !candidate.html.includes("tc-thinking__title--shimmer")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    fallbackSummarySnapshot.groupFoldTitles.some((title) => title.includes("Used 1 tool")),
    `expected the grouped transcript header to show the fallback count title first, got ${JSON.stringify(fallbackSummarySnapshot.groupFoldTitles)}`,
  );
  assert.equal(
    fallbackSummarySnapshot.progressRow,
    true,
    "expected the fallback-title gap to keep using the shared progress row",
  );

  await api.__testing.injectServeEvent({
    sessionId,
    summaryTitle: "Used 1 tool for checking the README",
    toolCallIds: ["streaming-context-tool-1"],
    turnIndex: 1,
    type: "turn.summary_updated",
  });
  const upgradedSummarySnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.groupFoldTitles.some((title) => title.includes("Used 1 tool for checking the README")) &&
      !candidate.html.includes("tc-thinking__title--shimmer")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    upgradedSummarySnapshot.groupFoldTitles.some((title) =>
      title.includes("Used 1 tool for checking the README")
    ),
    `expected turn.summary_updated to upgrade the folded transcript title, got ${JSON.stringify(upgradedSummarySnapshot.groupFoldTitles)}`,
  );
  assert.equal(
    upgradedSummarySnapshot.progressRow,
    true,
    "expected summary upgrades to leave the shared progress row lifecycle untouched",
  );

  await api.__testing.injectServeEvent({
    assistantMessageId: "streaming-context-group-1",
    assistantMessageEvent: {
      delta: "The README checks out.",
      kind: "content_delta",
    },
    message: {},
    sessionId,
    type: "message_update",
  });
  const resumedOutputSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.progressRow &&
      candidate.messageTexts.some((text) => text.includes("The README checks out."))
        ? candidate
        : undefined,
    20_000,
  );
  assert.equal(
    resumedOutputSnapshot.progressRow,
    true,
    "expected the shared progress row to stay mounted while assistant output is still streaming",
  );

  await api.__testing.injectServeEvent({
    messages: [],
    sessionId,
    type: "agent_end",
  });
  await api.__testing.injectServeEvent({
    sessionId,
    type: "agent_idle",
  });
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
      snapshot.loadingShimmerCount > 0
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
      snapshot.loadingShimmerCount === 0 &&
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
      snapshot.loadingShimmerCount === 0 &&
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
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.actionToolRowCount >= 1 &&
      candidate.toolTitles.some((title) => /Asked question/i.test(title)) &&
      candidate.html.includes('data-testid="answer-card"') &&
      candidate.html.includes("Deploy where?") &&
      candidate.html.includes("Staging")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    snapshot.actionToolRowCount >= 1,
    `expected the answered ask_question row to stay visible, got ${snapshot.actionToolRowCount}`,
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

  const toolEnd = await api.__testing.waitForEvent({
    timeoutMs: 20_000,
    type: "tool_execution_end",
  });
  const diffToolCallId =
    "toolCallId" in toolEnd && typeof toolEnd.toolCallId === "string"
      ? toolEnd.toolCallId
      : undefined;
  assert.ok(diffToolCallId, "expected tool_execution_end to include a toolCallId");
  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === activeSessionId &&
      candidate.actionToolRowCount >= 1 &&
      candidate.editDiffBadgeCount >= 1 &&
      /\+[0-9]+/.test(candidate.html) &&
      /-[0-9]+/.test(candidate.html)
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    snapshot.editDiffBadgeCount >= 1,
    `expected at least one edit diff badge, got ${snapshot.editDiffBadgeCount}`,
  );
  assert.match(snapshot.html, /\+[0-9]+/u);
  assert.match(snapshot.html, /-[0-9]+/u);
  assert.match(snapshot.html, /View diff/u);
  assert.match(snapshot.html, /before/u);
  assert.match(snapshot.html, /after/u);
  assert.doesNotMatch(snapshot.html, /Apply Edit/u);
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: { toolCallId: diffToolCallId },
      messageId: "webview-diff-open-intent",
      type: "openDiff",
    }),
  );
  const preparedChange = await waitForPreparedChange(
    api,
    diffToolCallId,
    (change) =>
      stripTerminalNewline(change.originalContent) === "before"
      && stripTerminalNewline(change.proposedContent) === "after",
  );
  assertPreparedChangeMatches(preparedChange, editFile, "before", "after");
  await waitForVisiblePreparedDiffEditors(diffToolCallId, 20_000);
  assert.equal(
    vscode.workspace
      .getConfiguration("diffEditor")
      .get<number>("renderSideBySideInlineBreakpoint"),
    0,
    "expected Tomcat to force a zero inline breakpoint so narrow diff editors stay side-by-side",
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    // Close the primary side bar (Explorer) so the diff editor spans the full
    // window width after the runtime breakpoint fix has already kept the diff in
    // a true left/right double-pane layout.
    await vscode.commands.executeCommand("workbench.action.closeSidebar");
    await pause(500);
    // Anchor to the always-present dev-host window title (the diff editor is the
    // active full-width editor here); a diff-tab-specific title does not reliably
    // resolve to window bounds and would skip the capture.
    captureTranscriptVisual("diff-double-pane", "window", "Extension Development Host");
    // Restore the Tomcat webview for the remainder of the flow.
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
  }
  assert.equal(await fs.readFile(editFile, "utf8"), "after\n");

  await api.__testing.injectServeEvent({
    assistantMessageId: "assistant-read-standalone",
    assistantMessageEvent: { delta: "Inspecting README.md", kind: "content_delta" },
    message: {},
    sessionId: activeSessionId,
    type: "message_update",
  });
  await api.__testing.injectServeEvent({
    args: { path: "README.md" },
    sessionId: activeSessionId,
    toolCallId: "tool-read-standalone",
    toolName: "read",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    display: { file: "README.md", kind: "file" },
    isError: false,
    result: "# readme\n",
    sessionId: activeSessionId,
    toolCallId: "tool-read-standalone",
    toolName: "read",
    type: "tool_execution_end",
  });
  const readSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === activeSessionId &&
      candidate.fileChipVisible &&
      candidate.toolTitles.some((title) => /Read/u.test(title)) &&
      candidate.html.includes("README.md")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(readSnapshot.fileChipVisible, "expected a standalone read row to render a file chip");
}

export async function assertWebviewRetryRecoveryFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const failureSummary = "API 错误 403 · aigateway.sunmi.com · Request-Id req-host-retry";
  const successText = "same session retry succeeded";
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-retry-recovery-session",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "retry 403 showcase",
      },
      messageId: "webview-retry-recovery-prompt",
      type: "prompt",
    }),
  );
  await api.__testing.waitForEvent({
    sessionId,
    timeoutMs: 20_000,
    type: "agent_end",
  });
  const failedSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
      && candidate.messageTexts.some((text) => text.includes(failureSummary))
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    failedSnapshot.messageTexts.some((text) => text.includes(failureSummary)),
    "expected the failed turn summary to render in the transcript",
  );

  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        sessionId,
        text: "retry 403 showcase",
      },
      messageId: "webview-retry-recovery-retry",
      type: "prompt",
    }),
  );
  await api.__testing.waitForEvent({
    sessionId,
    timeoutMs: 20_000,
    type: "agent_end",
  });
  const recoveredSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
      && candidate.messageTexts.some((text) => text.includes(successText))
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    recoveredSnapshot.messageTexts.some((text) => text.includes(successText)),
    "expected retrying in the same session to produce a successful assistant reply",
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    await pause(700);
    captureTranscriptVisual(
      "same-session-retry-success",
      "window",
      "Extension Development Host",
    );
  }

  await api.__testing.reloadWebview();
  await api.__testing.waitForWebviewReady();
  await waitForWebviewState(
    api,
    (state) => (state.activeSessionId === sessionId ? state : undefined),
    20_000,
  );
  const rehydratedSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
      && candidate.messageTexts.some((text) => text.includes(failureSummary))
      && candidate.messageTexts.some((text) => text.includes(successText))
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    rehydratedSnapshot.timelineKinds.includes("message:error"),
    `expected the rehydrated transcript to include an error bubble, got ${JSON.stringify(rehydratedSnapshot.timelineKinds)}`,
  );
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    await api.__testing.sendWebviewDomAction({
      edge: "top",
      kind: "scrollToEdge",
      testId: "stream-container",
    });
    await pause(700);
    captureTranscriptVisual(
      "transcript-error-record",
      "window",
      "Extension Development Host",
    );
  }
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
    5_000,
  ).catch(() => undefined);
  if (loading) {
    assert.ok(loading.historyLoaderVisible, "expected the subtle top loader while chasing the giant group");
    assert.equal(loading.toolRowCount, 0, "expected no partial tool rows while older pages are still loading");
  }

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
  assert.equal(
    restored.toolRowCount,
    0,
    "expected the recovered giant group to remain folded instead of rendering a half-loaded subset",
  );
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
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("selection-reference-codelens", "editor", "selection-context.ts");
  }

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
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("selection-reference-composer", "sidebar", "selection-context.ts");
  }

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "send-button",
  });
  await api.__testing.waitForEvent({ sessionId, timeoutMs: 30_000, type: "agent_end" });

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
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("selection-reference-history", "sidebar", "selection-context.ts");
  }
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
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("file-drop-reference-hover", "sidebar", "drop-context.md");
  }
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
  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    captureTranscriptVisual("file-drop-reference", "sidebar", "drop-context.md");
  }
}

export async function assertWebviewAtMentionReferenceFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-at-mention-reference-new-session",
  );
  await clearComposerDraft(api, sessionId);

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  assert.ok(workspaceRoot, "expected a real workspace folder for @ mention E2E");
  const scratchDir = path.join(
    workspaceRoot,
    `tmp-at-mention-file-${Date.now().toString(36)}`,
  );

  try {
    await fs.mkdir(scratchDir, { recursive: true });
    const stem = `at-mention-target-${Date.now().toString(36)}`;
    const filePath = path.join(scratchDir, `${stem}.ts`);
    await fs.writeFile(filePath, "export const atMentionTarget = true;\n", "utf8");
    const fileReference = await resolveUriToFileReference(vscode.Uri.file(filePath));

    await setComposerInputValue(api, `@${stem}`);

    await waitForContextSearchIntent(
      api,
      (intent) => intent.data.sessionId === sessionId && intent.data.query === stem,
      20_000,
    );

    await waitForWebviewDomSnapshot(
      api,
      (snapshot) =>
        snapshot.activeSessionId === sessionId &&
        snapshot.html.includes('data-testid="context-search-dropdown"') &&
        snapshot.html.includes(`title="${fileReference.path}"`)
          ? snapshot
          : undefined,
      20_000,
    ).catch((error: Error) => {
      throw new Error(`ATMENTION file dropdown stage failed: ${error.message}`);
    });

    await api.__testing.sendWebviewDomAction({
      index: 0,
      kind: "clickTestId",
      testId: "context-search-option",
    });

    const composerSnapshot = await waitForWebviewDomSnapshot(
      api,
      (snapshot) => {
        const chipCount = (snapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;
        const sendDisabled = /data-testid="send-button"[^>]*disabled/u.test(snapshot.html);
        return (
          snapshot.activeSessionId === sessionId &&
          chipCount === 1 &&
          snapshot.html.includes(`title="${fileReference.path}"`) &&
          !sendDisabled &&
          !snapshot.html.includes('data-testid="context-search-dropdown"')
        )
          ? snapshot
          : undefined;
      },
      20_000,
    ).catch((error: Error) => {
      throw new Error(`ATMENTION file chip stage failed: ${error.message}`);
    });
    assert.ok(
      composerSnapshot.html.includes(fileReference.label),
      "expected the composer chip label to match the selected @ file reference",
    );

    await api.__testing.sendWebviewDomAction({
      kind: "clickTestId",
      testId: "send-button",
    });
    await waitForEvent(api, { sessionId, type: "agent_end" });

    type RestoredReferenceSegment = {
      kind?: string;
      label?: string;
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
          (segment) =>
            segment.type === "reference" &&
            segment.kind === "file" &&
            segment.path === fileReference.path &&
            segment.lineStart == null &&
            segment.lineEnd == null,
        )
          ? { segments }
          : undefined;
      },
      20_000,
    );
    assert.ok(
      restoredMessage.segments?.some(
        (segment) =>
          segment.type === "reference" &&
          segment.path === fileReference.path &&
          segment.lineStart == null &&
          segment.lineEnd == null,
      ),
      "expected the sent @ file reference to stay line-free in user message segments",
    );

    await api.__testing.reloadWebview();
    const restoredSnapshot = await waitForWebviewDomSnapshot(
      api,
      (snapshot) =>
        snapshot.activeSessionId === sessionId &&
        snapshot.html.includes('data-testid="history-reference-chip"') &&
        snapshot.html.includes(`title="${fileReference.path}"`)
          ? snapshot
          : undefined,
      20_000,
    );
    assert.ok(
      restoredSnapshot.messageTexts.some((text) => text.includes(fileReference.label)),
      "expected the reloaded transcript to render the @ file reference chip",
    );
  } finally {
    await fs.rm(scratchDir, { force: true, recursive: true });
  }
}

export async function assertWebviewAtMentionDirectoryAndWarningFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-at-mention-directory-new-session",
  );
  await clearComposerDraft(api, sessionId);

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  assert.ok(workspaceRoot, "expected a real workspace folder for @ directory E2E");
  const scratchDir = path.join(
    workspaceRoot,
    `tmp-at-mention-scratch-${Date.now().toString(36)}`,
  );

  await fs.mkdir(scratchDir, { recursive: true });
  const dirStem = `directory-target-${Date.now().toString(36)}`;
  const dirPath = path.join(scratchDir, dirStem);
  await fs.mkdir(dirPath, { recursive: true });
  await fs.writeFile(path.join(dirPath, "nested.txt"), "nested\n", "utf8");
  const dirReference = await resolveUriToFileReference(vscode.Uri.file(dirPath));

  await setComposerInputValue(api, `@${dirReference.label}`);
  await waitForContextSearchIntent(
    api,
    (intent) => intent.data.sessionId === sessionId && intent.data.query === dirReference.label,
    20_000,
  );

  await waitForWebviewDomSnapshot(
    api,
    (snapshot) =>
      snapshot.activeSessionId === sessionId &&
      snapshot.html.includes('data-testid="context-search-dropdown"') &&
      snapshot.html.includes(`title="${dirReference.path}"`)
        ? snapshot
        : undefined,
    20_000,
  ).catch((error: Error) => {
    throw new Error(`ATMENTION directory dropdown stage failed: ${error.message}`);
  });

  await api.__testing.sendWebviewDomAction({
    index: 0,
    kind: "clickTestId",
    testId: "context-search-option",
  });

  const directorySnapshot = await waitForWebviewDomSnapshot(
    api,
    (snapshot) => {
      const chipCount = (snapshot.html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;
      return (
        snapshot.activeSessionId === sessionId &&
        chipCount === 1 &&
        snapshot.html.includes(`title="${dirReference.path}"`) &&
        snapshot.html.includes(dirReference.label)
      )
        ? snapshot
        : undefined;
    },
    20_000,
  ).catch((error: Error) => {
    throw new Error(`ATMENTION directory chip stage failed: ${error.message}`);
  });
  assert.ok(
    directorySnapshot.html.includes(dirReference.label),
    "expected the composer chip label to preserve the directory suffix",
  );

  const originalShowWarningMessage = vscode.window.showWarningMessage;
  const warnings: string[] = [];
  Object.defineProperty(vscode.window, "showWarningMessage", {
    configurable: true,
    value: async (message: string, ..._items: string[]) => {
      warnings.push(message);
      return undefined;
    },
  });

  try {
    await api.__testing.reloadWebview();
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    const warningSessionId = await claimActiveWebviewSession(
      api,
      "webview-at-mention-warning-claim",
      20_000,
    );
    await clearComposerDraft(api, warningSessionId);

    await setComposerInputValue(api, "@workspace-missing");
    const searchIntent = await waitForContextSearchIntent(
      api,
      (intent) =>
        intent.data.sessionId === warningSessionId &&
        intent.data.query === "workspace-missing",
      20_000,
    );

    await api.__testing.sendWebviewHostEvent({
      matches: [],
      query: searchIntent.data.query,
      requestId: searchIntent.data.requestId,
      sessionId: warningSessionId,
      truncated: false,
      type: "contextSearchResult",
      workspaceAvailable: false,
    });

    await waitForWebviewDomSnapshot(
      api,
      (snapshot) =>
        snapshot.activeSessionId === warningSessionId &&
        snapshot.html.includes('data-testid="composer"') &&
        !snapshot.html.includes('data-testid="context-search-dropdown"')
          ? snapshot
          : undefined,
      20_000,
    ).catch((error: Error) => {
      throw new Error(`ATMENTION warning stage failed: ${error.message}`);
    });
    assert.equal(
      warnings.at(-1),
      "打开文件夹后可用 @",
      "expected the no-workspace @ warning to be surfaced once the host reports workspaceAvailable=false",
    );
  } finally {
    await fs.rm(scratchDir, { force: true, recursive: true });
    Object.defineProperty(vscode.window, "showWarningMessage", {
      configurable: true,
      value: originalShowWarningMessage,
    });
  }
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

export async function assertWebviewSessionTitleFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();

  const plainSessionId = await createFreshWebviewSession(
    api,
    "webview-session-title-plain-new-session",
  );
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        segments: [{ text: "hello", type: "text" }],
        sessionId: plainSessionId,
        text: "hello",
      },
      messageId: "webview-session-title-plain-prompt",
      type: "prompt",
    }),
  );
  const plainTitle = await waitForWebviewState(
    api,
    (state) =>
      state.sessions.find((session) => session.sessionId === plainSessionId)?.title === "hello"
        ? "hello"
        : undefined,
    20_000,
  );
  assert.equal(plainTitle, "hello");
  assert.ok(
    api.__testing.getObservedEvents().some(
      (event) => event.sessionId === plainSessionId && event.type === "session.title_updated",
    ),
    "expected a session.title_updated event for the pure-text first message",
  );

  api.__testing.clearObservedEvents();
  const referencedSessionId = await createFreshWebviewSession(
    api,
    "webview-session-title-reference-new-session",
  );
  const workspaceDir = requireEnv(TEST_DEFAULT_CWD_ENV);
  const filePath = path.join(workspaceDir, "title-context.ts");
  await fs.writeFile(filePath, "export const titleContext = true;\n", "utf8");
  await api.__testing.sendWebviewIntent(
    buildWebviewIntent({
      data: {
        segments: [
          { text: "before ", type: "text" },
          {
            kind: "file",
            label: "title-context.ts",
            path: filePath,
            type: "reference",
          },
          { text: "after", type: "text" },
        ],
        sessionId: referencedSessionId,
        text: "before title-context.ts after",
      },
      messageId: "webview-session-title-reference-prompt",
      type: "prompt",
    }),
  );
  const referencedTitle = await waitForWebviewState(
    api,
    (state) => {
      const title = state.sessions.find((session) => session.sessionId === referencedSessionId)?.title;
      return title && title !== "New session" ? title : undefined;
    },
    20_000,
  );
  assert.equal(referencedTitle, "before after");
  assert.ok(
    api.__testing.getObservedEvents().some(
      (event) => event.sessionId === referencedSessionId && event.type === "session.title_updated",
    ),
    "expected a session.title_updated event for the reference-bearing first message",
  );
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
    | "diff-double-pane"
    | "expanded"
    | "file-drop-reference"
    | "file-drop-reference-hover"
    | "file-chip"
    | "model-dropdown-open"
    | "progress"
    | "reload-replay"
    | "rich-render"
    | "same-session-retry-success"
    | "selection-reference-codelens"
    | "selection-reference-composer"
    | "selection-reference-history"
    | "settings-alignment"
    | "switch-order"
    | "switch-restore"
    | "todo-expanded"
    | "transcript-error-record"
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
    candidate.actionToolRowCount >= 1 &&
    candidate.planCardCount >= 1 &&
    !candidate.progressRow &&
    !candidate.todoWidgetVisible &&
    candidate.planFooterSameRow &&
    candidate.userPromptPill &&
    candidate.assistantNoCard &&
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
        candidate.progressRow &&
        candidate.todoWidgetVisible &&
        candidate.planCardCount > 0 &&
        candidate.planFooterSameRow &&
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
        true,
        "expected the inline progress row to stay visible while the docked todo widget owns the busy state",
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
    collapsed.actionToolRowCount >= 1,
    `expected at least one standalone action tool row before expanding context, got ${collapsed.actionToolRowCount}`,
  );
  assert.equal(
    collapsed.commandBlockCount,
    1,
    `expected the successful bash showcase row to render once, got ${collapsed.commandBlockCount}`,
  );
  assert.ok(
    !collapsed.expandedToolTitles.some((title) => title.includes("Ran git status --short")),
    `expected the successful bash showcase row to stay collapsed, got ${JSON.stringify(collapsed.expandedToolTitles)}`,
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
  assert.ok(
    collapsed.groupFoldTitles.some((title) => title.includes("Reviewed 1 file")),
    `expected the first context segment to fold into "Reviewed 1 file", got ${JSON.stringify(collapsed.groupFoldTitles)}`,
  );
  assert.ok(
    collapsed.toolTitles.some((title) => title.includes('Searched "vscode chat thinking collapsible"')),
    `expected the trailing single web_search context tool to render as a standalone row, got ${JSON.stringify(collapsed.toolTitles)}`,
  );
  assert.equal(
    collapsed.toolRowCount,
    2,
    `expected the standalone bash row plus the trailing single web_search context row while context stays folded, got ${collapsed.toolRowCount}`,
  );
  assert.equal(
    collapsed.toolCardCount,
    0,
    `expected no tool-call cards after grouping fix, got ${collapsed.toolCardCount}`,
  );
  assert.ok(
    collapsed.html.includes("git status --short"),
    "expected the standalone bash action row to keep its command chip visible",
  );
  assert.ok(
    collapsed.html.includes("plans/transcript-ui-showcase.plan.md"),
    "expected the collapsed bash card to show a tail preview of command output",
  );
  assert.equal(
    (collapsed.html.match(/git status --short/g) ?? []).length,
    1,
    "expected the standalone bash command to render once without duplicate fold titles",
  );
}

export async function assertTranscriptRichRenderingFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await waitForWebviewBootstrapSettled(api);
  const sessionId = await createFreshWebviewSession(api, "webview-rich-render-session");
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  assert.ok(workspaceRoot, "expected a workspace root for transcript rich-render E2E");

  const richFilePath = path.join(workspaceRoot, "src", "test", "fixtures", "rich-render.ts");
  await fs.mkdir(path.dirname(richFilePath), { recursive: true });
  await fs.writeFile(
    richFilePath,
    [
      "export function richRenderFixture() {",
      '  return "line two";',
      "}",
      "",
      "export function otherLine() {",
      "  return 42;",
      "}",
      "",
    ].join("\n"),
    "utf8",
  );

  await api.__testing.hydrateWebviewHistory({
    messages: [
      {
        id: "user-rich-render",
        message: {
          content: "Render the streamed markdown sample.",
          role: "user",
        },
        type: "message",
      },
    ],
    sessionId,
  });
  await api.__testing.applyWebviewSessionState({
    busy: true,
    model: "gpt-5.4",
    sessionId,
  });

  const contentDeltas = [
    "## Fix ",
    "plan\n\nStart with `src/test/fixtures/missing-link.ts:9`, ",
    "then compare the snippet below.\n\n```ts src/test/fixtures/rich-render.ts:6\n",
    "export function otherLine() {\n  return 42;\n}\n```\n\n```text\n",
    "A --> B --> C\n```\n\n```mermaid\nflowchart LR\n",
    "  Start --> Finish\n```",
  ];
  let streamingHighlightSnapshot: { html: string } | null = null;
  for (const [index, delta] of contentDeltas.entries()) {
    await api.__testing.injectServeEvent({
      assistantMessageEvent: {
        delta,
        kind: "content_delta",
      },
      assistantMessageId: "assistant-rich-render",
      message: {},
      sessionId,
      type: "message_update",
    });
    await pause(30);
    if (index === 3) {
      streamingHighlightSnapshot = await waitForWebviewDomSnapshot(
        api,
        (candidate) =>
          candidate.activeSessionId === sessionId &&
          /hljs-[\w-]+/u.test(candidate.html) &&
          candidate.html.includes(">rich-render.ts:6<") &&
          !candidate.html.includes('data-testid="plan-mermaid"')
            ? candidate
            : undefined,
        20_000,
      );
    }
  }
  assert.ok(streamingHighlightSnapshot, "expected a streaming snapshot after the first code block completed");
  assert.match(streamingHighlightSnapshot.html, /hljs-[\w-]+/u);
  assert.doesNotMatch(streamingHighlightSnapshot.html, /data-testid="plan-mermaid"/u);
  await api.__testing.injectServeEvent({
    assistantMessageEvent: {
      delta: "## Inspect\n\nStart with `src/thinking/plain.ts:9`.",
      kind: "thinking_delta",
    },
    assistantMessageId: "assistant-rich-render",
    message: {},
    sessionId,
    type: "message_update",
  });
  await api.__testing.injectServeEvent({
    assistantMessageId: "assistant-rich-render",
    message: {},
    sessionId,
    type: "message_end",
  });
  await api.__testing.applyWebviewSessionState({
    busy: false,
    model: "gpt-5.4",
    sessionId,
  });

  const snapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.assistantCodeCardCount >= 2 &&
      candidate.assistantClickablePathCount >= 1 &&
      candidate.html.includes("assistant-code-copy") &&
      candidate.html.includes("Fix plan")
        ? candidate
        : undefined,
    20_000,
  );
  assert.ok(
    snapshot.assistantCodeCardCount >= 2,
    `expected at least two assistant code cards, got ${snapshot.assistantCodeCardCount}`,
  );
  assert.ok(
    snapshot.assistantClickablePathCount >= 1,
    `expected at least one clickable assistant inline path, got ${snapshot.assistantClickablePathCount}`,
  );
  assert.match(snapshot.html, /assistant-code-copy/u);
  assert.match(snapshot.html, />rich-render\.ts:6</u);
  assert.match(snapshot.html, /title="src\/test\/fixtures\/rich-render\.ts:6"/u);
  assert.match(snapshot.html, />missing-link\.ts:9</u);
  assert.match(snapshot.html, /title="src\/test\/fixtures\/missing-link\.ts:9"/u);
  assert.match(snapshot.html, /A --&gt; B --&gt; C/u);
  assert.match(snapshot.html, /tc-code-card--bare/u);
  assert.doesNotMatch(snapshot.html, /tc-code-card__lang/u);
  const settledSnapshot = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
      && /hljs-[\w-]+/u.test(candidate.html)
      && candidate.html.includes('data-testid="plan-mermaid"')
      && candidate.html.includes("<svg")
        ? candidate
        : undefined,
    20_000,
  );
  assert.equal(
    settledSnapshot.assistantCodeCardCount,
    snapshot.assistantCodeCardCount,
    "expected assistant code-card count to stay stable across consecutive DOM snapshots",
  );
  assert.equal(
    settledSnapshot.assistantClickablePathCount,
    snapshot.assistantClickablePathCount,
    "expected assistant inline-path count to stay stable across consecutive DOM snapshots",
  );
  assert.equal(
    (settledSnapshot.html.match(/assistant-code-copy/g) ?? []).length,
    (snapshot.html.match(/assistant-code-copy/g) ?? []).length,
    "expected copy-button structure to stay stable across consecutive DOM snapshots",
  );
  assert.equal(
    (settledSnapshot.html.match(/tc-code-card__header/g) ?? []).length,
    1,
    "expected only file-backed code blocks to render a header",
  );
  assert.match(settledSnapshot.html, /hljs-[\w-]+/u);
  assert.match(settledSnapshot.html, /data-testid="plan-mermaid"/u);
  assert.match(settledSnapshot.html, /<svg/u);

  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "assistant-code-file",
  });
  const fileCardEditor = await waitForActiveTextEditor(
    (editor) =>
      editor?.document.uri.fsPath === richFilePath && editor.selection.start.line === 5,
  );
  assert.equal(fileCardEditor.selection.start.line, 5, "expected code-card click to reveal line 6");

  api.__testing.clearObservedEvents();
  await api.__testing.focusWebview();
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "assistant-clickable-path",
  });
  const afterBrokenInlinePath = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId && candidate.fileChipOpen
        ? candidate
        : undefined,
    10_000,
  );
  assert.equal(
    afterBrokenInlinePath.messageTexts.length,
    settledSnapshot.messageTexts.length,
    "expected a broken assistant inline path to avoid appending a new transcript error message",
  );
  assert.doesNotMatch(afterBrokenInlinePath.html, /Unable to open file/u);
  assert.equal(
    afterBrokenInlinePath.assistantClickablePathCount,
    settledSnapshot.assistantClickablePathCount,
    "expected broken-link handling to leave clickable-path structure unchanged",
  );

  await api.__testing.focusWebview();
  await api.__testing.sendWebviewDomAction({
    kind: "clickTestId",
    testId: "thinking-toggle",
  });
  const expandedThinking = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId
      && candidate.html.includes("## Inspect")
      && candidate.html.includes("src/thinking/plain.ts:9")
        ? candidate
        : undefined,
    10_000,
  );
  assert.equal(
    expandedThinking.assistantClickablePathCount,
    1,
    "expected only assistant-body inline paths to remain clickable after thinking expands as plain text",
  );

  if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
    await api.__testing.focusWebview();
    captureTranscriptVisual("rich-render");
  }
}

export async function assertWebviewPlanToolUxFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-plan-tool-ux-session",
  );
  const planId = "plan-tool-ux";
  const planPath = `/tmp/${planId}.plan.md`;
  const renderPlanMarkdown = (
    todos: Array<{ content: string; id: string; status: "completed" | "pending" }>,
  ) =>
    [
      "---",
      `plan_id: ${planId}`,
      "name: Plan tool ux",
      "overview: Keep one create card and many update rows.",
      "state: planning",
      "todos:",
      ...todos.flatMap((todo) => [
        `- id: ${todo.id}`,
        `  content: ${todo.content}`,
        `  status: ${todo.status}`,
      ]),
      "---",
      "",
      "# Plan tool ux",
      "",
      "Keep one create card and many update rows.",
      "",
    ].join("\n");
  const initialTodos = [
    { content: "Draft the immutable create card", id: "todo-1", status: "pending" as const },
    { content: "Render update rows", id: "todo-2", status: "pending" as const },
    { content: "Guard against drift", id: "todo-3", status: "pending" as const },
  ];
  const afterFirstUpdateTodos = [
    { content: "Draft the immutable create card", id: "todo-1", status: "completed" as const },
    { content: "Render update rows", id: "todo-2", status: "pending" as const },
    { content: "Guard against drift", id: "todo-3", status: "pending" as const },
  ];
  const afterSecondUpdateTodos = [
    { content: "Draft the immutable create card", id: "todo-1", status: "completed" as const },
    { content: "Render update rows", id: "todo-2", status: "completed" as const },
    { content: "Guard against drift", id: "todo-3", status: "pending" as const },
  ];
  await fs.mkdir(path.dirname(planPath), { recursive: true });
  await fs.writeFile(planPath, renderPlanMarkdown(initialTodos), "utf8");

  await api.__testing.injectServeEvent({
    args: {
      draft: "Keep one create card and many update rows.",
      goal: "Plan tool ux",
      todos: initialTodos,
    },
    sessionId,
    toolCallId: "plan-create-1",
    toolName: "create_plan",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    path: planPath,
    planId,
    sessionId,
    state: "planning",
    type: "plan.create",
  });
  await api.__testing.injectServeEvent({
    planId,
    sessionId,
    todos: initialTodos,
    type: "plan.todos",
  });

  const pending = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.planCardCount === 1 &&
      candidate.html.includes('data-testid="view-plan-pending"')
        ? candidate
        : undefined,
    5_000,
  );
  assert.equal(
    pending.planCardCount,
    1,
    `expected the legacy pending plan card before create_plan completes, got ${pending.planCardCount}`,
  );
  assert.ok(
    pending.html.includes('data-testid="view-plan-pending"'),
    "expected the pending create_plan card to show the dot-state footer",
  );

  await api.__testing.injectServeEvent({
    isError: false,
    result: JSON.stringify({
      path: planPath,
      plan_id: planId,
      state: "planning",
    }),
    sessionId,
    toolCallId: "plan-create-1",
    toolName: "create_plan",
    type: "tool_execution_end",
  });

  await api.__testing.injectServeEvent({
    args: {
      ops: [{ kind: "set_status", status: "completed", todo_id: "todo-1" }],
      path: planPath,
      plan_id: planId,
    },
    sessionId,
    toolCallId: "plan-update-1",
    toolName: "update_plan",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    isError: false,
    result: JSON.stringify({
      applied: 1,
      items: [
        { id: "todo-1", status: "completed" },
        { id: "todo-2", status: "pending" },
        { id: "todo-3", status: "pending" },
      ],
      path: planPath,
      plan_id: planId,
      plan_state_after: "planning",
      plan_state_before: "planning",
    }),
    sessionId,
    toolCallId: "plan-update-1",
    toolName: "update_plan",
    type: "tool_execution_end",
  });
  await api.__testing.injectServeEvent({
    planId,
    sessionId,
    todos: afterFirstUpdateTodos,
    type: "plan.todos",
  });
  await fs.writeFile(planPath, renderPlanMarkdown(afterFirstUpdateTodos), "utf8");

  await api.__testing.injectServeEvent({
    args: {
      ops: [{ kind: "set_status", status: "completed", todo_id: "todo-2" }],
      path: planPath,
      plan_id: planId,
    },
    sessionId,
    toolCallId: "plan-update-2",
    toolName: "update_plan",
    type: "tool_execution_start",
  });
  await api.__testing.injectServeEvent({
    isError: false,
    result: JSON.stringify({
      applied: 1,
      items: [
        { id: "todo-1", status: "completed" },
        { id: "todo-2", status: "completed" },
        { id: "todo-3", status: "pending" },
      ],
      path: planPath,
      plan_id: planId,
      plan_state_after: "planning",
      plan_state_before: "planning",
    }),
    sessionId,
    toolCallId: "plan-update-2",
    toolName: "update_plan",
    type: "tool_execution_end",
  });
  await api.__testing.injectServeEvent({
    planId,
    sessionId,
    todos: afterSecondUpdateTodos,
    type: "plan.todos",
  });
  await fs.writeFile(planPath, renderPlanMarkdown(afterSecondUpdateTodos), "utf8");

  const settled = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.planCardCount === 1 &&
      candidate.toolTitles.some((title) => title.includes("Checked 1 · 1/3")) &&
      candidate.toolTitles.some((title) => title.includes("Checked 1 · 2/3")) &&
      candidate.html.includes("View Plan")
        ? candidate
        : undefined,
    20_000,
  );
  assert.equal(settled.planCardCount, 1, `expected a single plan card, got ${settled.planCardCount}`);
  assert.ok(
    settled.html.includes("Plan tool ux"),
    "expected the completed create_plan card to keep the original title",
  );
  assert.equal(
    settled.planCardTodoCountText,
    "3 todos",
    `expected the pinned create_plan card to keep its original todo count, got ${settled.planCardTodoCountText}`,
  );
  assert.equal(
    settled.toolTitles.filter((title) => title.includes("Checked 1 ·")).length,
    2,
    `expected two standalone update_plan event rows, got ${JSON.stringify(settled.toolTitles)}`,
  );
  assert.ok(
    !settled.toolTitles.some((title) => title.includes("Creating plan")),
    `expected the completed create_plan row to promote into the plan card, got ${JSON.stringify(settled.toolTitles)}`,
  );
  assert.ok(
    !settled.groupFoldTitles.some(
      (title) =>
        title.includes("Creating plan")
        || title.includes("Checked 1")
        || title.includes("Updated plan"),
    ),
    `expected no folded thinking header to echo plan tool labels, got ${JSON.stringify(settled.groupFoldTitles)}`,
  );
  assert.ok(
    settled.html.includes("View Plan"),
    "expected View Plan to return after the plan tool finishes",
  );
  await api.__testing.openPlanPreview(planPath);
  const preview = await waitForPlanPreviewDom(
    api,
    planPath,
    (snapshot) =>
      snapshot.bodyHasContent &&
      snapshot.todoItemCount === 3 &&
      snapshot.todoStatuses.join(",") === "completed,completed,pending",
  );
  assert.deepEqual(
    preview.todoStatuses,
    ["completed", "completed", "pending"],
    `expected the plan preview to reflect the latest todo statuses, got ${JSON.stringify(preview.todoStatuses)}`,
  );
}

export async function assertWebviewStickyHistoryFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  api.__testing.clearObservedEvents();
  const sessionId = await createFreshWebviewSession(
    api,
    "webview-sticky-history-session",
  );

  const prompts = Array.from(
    { length: 20 },
    (_, index) => `第${index + 1}轮 sticky 问题`,
  );
  for (const [index, text] of prompts.entries()) {
    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent(
      buildWebviewIntent({
        data: { sessionId, text },
        messageId: `webview-sticky-history-prompt-${index + 1}`,
        type: "prompt",
      }),
    );
    await waitForEvent(api, { type: "agent_end" });
  }

  await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.messageTexts.includes(prompts[0]) &&
      candidate.messageTexts.includes(prompts[1]) &&
      candidate.messageTexts.includes(prompts[prompts.length - 1])
        ? candidate
        : undefined,
  );

  await api.__testing.sendWebviewDomAction({
    index: 2,
    kind: "scrollIntoView",
    scrollBlock: "start",
    testId: "message-block",
  });
  await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId && !candidate.stickyPromptText
        ? candidate
        : undefined,
  );

  await api.__testing.sendWebviewDomAction({
    index: 3,
    kind: "scrollIntoView",
    scrollBlock: "start",
    testId: "message-block",
  });
  const historicalTurn = await waitForWebviewDomSnapshot(
    api,
    (candidate) =>
      candidate.activeSessionId === sessionId &&
      candidate.stickyPromptText !== null &&
      candidate.stickyPromptText !== prompts[prompts.length - 1]
        ? candidate
        : undefined,
  );
  assert.ok(
    prompts.slice(0, -1).includes(historicalTurn.stickyPromptText ?? ""),
    `expected sticky prompt to switch to a historical turn, got ${historicalTurn.stickyPromptText}`,
  );
}

type PlanPreviewDomSnapshot = Awaited<
  ReturnType<TomcatExtensionApi["__testing"]["capturePlanPreviewDom"]>
>;

async function waitForPlanPreviewDom(
  api: TomcatExtensionApi,
  planPath: string,
  predicate: (snapshot: PlanPreviewDomSnapshot) => boolean,
  timeoutMs = 30_000,
): Promise<PlanPreviewDomSnapshot> {
  const startedAt = Date.now();
  let lastError: unknown;
  let lastSnapshot: PlanPreviewDomSnapshot | undefined;
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const snapshot = await api.__testing.capturePlanPreviewDom(planPath);
      lastSnapshot = snapshot;
      if (predicate(snapshot)) {
        return snapshot;
      }
    } catch (error) {
      lastError = error;
    }
    await pause(200);
  }
  throw new Error(
    `Timed out waiting for plan preview DOM. lastSnapshot=${JSON.stringify(lastSnapshot)} lastError=${String(lastError)}`,
  );
}

/** Poll until the active text editor satisfies `predicate` (used for Markdown → native). */
async function waitForActiveTextEditor(
  predicate: (editor: vscode.TextEditor | undefined) => boolean,
  timeoutMs = 20_000,
): Promise<vscode.TextEditor> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const editor = vscode.window.activeTextEditor;
    if (predicate(editor) && editor) {
      return editor;
    }
    await pause(200);
  }
  throw new Error(
    `Timed out waiting for the active text editor. active=${vscode.window.activeTextEditor?.document.uri.fsPath ?? "none"}`,
  );
}

/**
 * The one automated check of the real `.plan.md` custom editor resolve + webview:
 * open a plan file, verify the hybrid (default B) in-body action strip is a fixed
 * header and the Preview four-state checklist renders, open the raw file in the
 * native text editor via the "Markdown" title-bar command, return via "Preview",
 * hot-reload on document edits, persist the build model, add a selection to chat
 * via both entry points (right-click command + floating button), then regress the
 * native (A) toolbar style.
 */
export async function assertPlanPreviewCustomEditorFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  assert.ok(workspaceRoot, "expected a real workspace folder for the plan preview E2E");

  // A live chat session must exist so "add selection to chat" can insert a chip.
  // The chip lands in whichever session `focusWebviewSurface()` reveals (the
  // sidebar's active session), so we don't pin assertions to this id.
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const sessionId = await createFreshWebviewSession(api, "plan-preview-selection-session");

  const scratchDir = path.join(
    workspaceRoot,
    `tmp-plan-preview-${Date.now().toString(36)}`,
  );
  const planBasename = "e2e-preview.plan.md";
  const planPath = path.join(scratchDir, planBasename);
  const linkedFilePath = path.join(scratchDir, "fixtures", "preview-target.ts");
  const linkedDisplayPath = path
    .relative(workspaceRoot, linkedFilePath)
    .split(path.sep)
    .join("/");
  const planUri = vscode.Uri.file(planPath);
  const planId = `e2e-plan-${Date.now().toString(36)}`;
  const fillerParagraphs = Array.from(
    { length: 24 },
    (_, index) => `Scroll filler paragraph ${index + 1}.`,
  );
  const initialText = [
    "---",
    `plan_id: ${planId}`,
    "name: E2E Plan Preview",
    "overview: Verify the custom editor renders",
    "state: planning",
    "todos:",
    "- id: t1",
    "  content: First task",
    "  status: completed",
    "- id: t2",
    "  content: Second task",
    "  status: in_progress",
    "- id: t3",
    "  content: Third task",
    "  status: pending",
    "---",
    "",
    "# E2E heading",
    "",
    "Body paragraph for the preview.",
    "",
    `Open \`${linkedDisplayPath}:2\` before building.`,
    "",
    "```mermaid",
    "flowchart LR",
    "  a[Start] --> b[Finish]",
    "```",
    "",
    ...fillerParagraphs.flatMap((line) => [line, ""]),
  ].join("\n");

  const chipCount = (html: string): number =>
    (html.match(/data-testid="composer-reference-chip"/gu) ?? []).length;

  try {
    await fs.mkdir(scratchDir, { recursive: true });
    await fs.mkdir(path.dirname(linkedFilePath), { recursive: true });
    await fs.writeFile(
      linkedFilePath,
      [
        "export function previewTarget() {",
        "  return 'ready';",
        "}",
      ].join("\n"),
      "utf8",
    );
    await fs.writeFile(planPath, initialText, "utf8");

    await api.__testing.injectServeEvent({
      path: planPath,
      planId,
      sessionId,
      state: "planning",
      type: "plan.create",
    });
    await pause(400);
    await assert.rejects(
      () => api.__testing.capturePlanPreviewDom(planPath),
      /No plan preview panel is open/u,
      "expected plan.create alone to record the path but not open the preview before review completes",
    );
    await api.__testing.injectServeEvent({
      planId,
      sessionId,
      summary: "Tomcat plan review: looks good",
      type: "plan.review",
    });

    // Default is now hybrid (B): the slim in-body action strip is present.
    const preview = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) =>
        snapshot.bodyHasContent &&
        snapshot.toolbarStyle === "hybrid" &&
        snapshot.todoItemCount === 3,
    );
    assert.equal(
      preview.hasActionStrip,
      true,
      "expected the hybrid in-body action strip by default",
    );
    assert.equal(
      preview.stripOutsideContent,
      true,
      "expected the action strip to be a fixed header outside the scrolling content",
    );
    assert.ok(
      preview.stripInsetLeft !== null && preview.stripInsetLeft <= 2,
      `expected the action strip to span the full editor width (stripInsetLeft ~0), got ${String(preview.stripInsetLeft)}`,
    );
    assert.ok(
      preview.bodyInsetLeft !== null && preview.bodyInsetLeft >= 12,
      `expected the rendered body to keep left/right breathing room (bodyInsetLeft >= 12), got ${String(preview.bodyInsetLeft)}`,
    );
    assert.equal(
      preview.todoCountText,
      "3 To-dos",
      `expected a "3 To-dos" count header, got ${preview.todoCountText}`,
    );
    assert.equal(preview.bodyHasContent, true, "expected the rendered body to have content");

    // The ```mermaid``` fence renders to an inline SVG diagram (lazy-loaded).
    const mermaid = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) => snapshot.mermaidSvgCount >= 1,
      20_000,
    );
    assert.ok(
      mermaid.mermaidSvgCount >= 1,
      `expected at least one rendered mermaid SVG, got ${mermaid.mermaidSvgCount}`,
    );

    const inlinePathSnapshot = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) => snapshot.inlinePathCount >= 1,
      20_000,
    );
    assert.ok(
      inlinePathSnapshot.inlinePathCount >= 1,
      `expected at least one clickable inline path in the plan preview, got ${inlinePathSnapshot.inlinePathCount}`,
    );
    api.__testing.clearObservedFileOpens();
    await api.__testing.dispatchPlanPreviewDomAction(planPath, {
      kind: "clickSelector",
      selector: ".tc-inline-path",
    });
    await waitFor(
      () =>
        api.__testing
          .getObservedFileOpens()
          .some((entry) => entry.path === linkedDisplayPath && entry.line === 2),
      20_000,
      `expected the inline path click to call ide.showFile(${linkedDisplayPath}, 2)`,
    );
    const linkedEditor = await waitForActiveTextEditor(
      (editor) => editor?.document.uri.fsPath === linkedFilePath,
    );
    assert.equal(
      linkedEditor.document.uri.fsPath,
      linkedFilePath,
      `expected the inline path click to open ${linkedDisplayPath}`,
    );
    await api.__testing.openPlanPreview(planPath);
    await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.bodyHasContent);

    // Native title-bar command → "Markdown" opens the plain native text editor
    // for the same file (no in-webview source view any more).
    await api.__testing.executeCommand("tomcat.plan.viewAsMarkdown");
    const nativeEditor = await waitForActiveTextEditor(
      (editor) => editor?.document.uri.fsPath === planPath,
    );
    assert.ok(
      nativeEditor.document.getText().includes("# E2E heading"),
      "expected 'Markdown' to open the raw plan file in the native text editor",
    );

    // "Preview" from the native editor reopens the custom preview; hot-update the
    // document afterwards via disk write + serve event, and expect the checklist
    // to grow without kicking the user back to the top of the scroll column.
    await api.__testing.executeCommand("tomcat.plan.viewAsPreview");
    await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.bodyHasContent);
    await api.__testing.dispatchPlanPreviewDomAction(planPath, {
      kind: "setContentScrollTop",
      scrollTop: 280,
    });
    const scrollBeforeHotUpdate = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) => (snapshot.contentScrollTop ?? 0) >= 200,
    );

    const updatedText = initialText.replace(
      "---\n\n# E2E heading",
      ["- id: t4", "  content: Fourth task", "  status: cancelled", "---", "", "# E2E heading"].join(
        "\n",
      ),
    );
    await fs.writeFile(planPath, updatedText, "utf8");
    await api.__testing.injectServeEvent({
      path: planPath,
      planId,
      sessionId,
      state: "planning",
      type: "plan.update",
    });

    // Expected 1-based source lines of the rendered blocks, derived from the
    // (post hot-update) document so the assertions can't drift.
    const updatedLines = updatedText.split("\n");
    const headingLine = updatedLines.indexOf("# E2E heading") + 1;
    const paragraphLine = updatedLines.indexOf("Body paragraph for the preview.") + 1;
    const headingChip = `${planBasename}:${headingLine}`;
    const paragraphChip = `${planBasename}:${paragraphLine}`;

    const hotUpdated = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) => snapshot.bodyHasContent && snapshot.todoItemCount === 4,
    );
    assert.equal(
      hotUpdated.todoCountText,
      "4 To-dos",
      `expected the count header to hot-update to "4 To-dos", got ${hotUpdated.todoCountText}`,
    );
    assert.ok(
      hotUpdated.contentScrollTop !== null
      && scrollBeforeHotUpdate.contentScrollTop !== null
      && Math.abs(hotUpdated.contentScrollTop - scrollBeforeHotUpdate.contentScrollTop) <= 32,
      `expected hot-update to preserve the reading position, before=${String(scrollBeforeHotUpdate.contentScrollTop)} after=${String(hotUpdated.contentScrollTop)}`,
    );

    // When the serve exposes ready models, selecting one on the hybrid strip
    // persists it to the global config.
    if (hotUpdated.buildModelOptions.length > 0) {
      const targetModel = hotUpdated.buildModelOptions[0];
      await api.__testing.dispatchPlanPreviewDomAction(planPath, {
        kind: "selectBuildModel",
        modelId: targetModel,
      });
      await waitForPlanPreviewDom(
        api,
        planPath,
        (snapshot) => snapshot.buildModelValue === targetModel,
      );
      const persisted = vscode.workspace
        .getConfiguration("tomcat")
        .get<string>("plan.buildModel", "");
      assert.equal(
        persisted,
        targetModel,
        `expected tomcat.plan.buildModel to persist ${targetModel}, got ${persisted}`,
      );
      await vscode.workspace
        .getConfiguration("tomcat")
        .update("plan.buildModel", "", vscode.ConfigurationTarget.Global);
    }

    // Baseline chip count before adding any selection to the chat.
    const baseline = await api.__testing.captureWebviewDom();
    const baseChips = chipCount(baseline.html);

    // Selection → chat, path 1: the right-click command captures the live
    // selection (heading) inside the focused plan editor. The chip must carry
    // the exact source line derived from the block's data-source-line attribute.
    await api.__testing.openPlanPreview(planPath);
    await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.bodyHasContent);
    await api.__testing.dispatchPlanPreviewDomAction(planPath, {
      kind: "selectText",
      selector: ".tc-plan-preview__body h1",
    });
    await api.__testing.executeCommand(TOMCAT_PLAN_ADD_SELECTION_TO_CHAT_COMMAND);
    const afterCommand = await waitForWebviewDomSnapshot(
      api,
      (snapshot) =>
        chipCount(snapshot.html) === baseChips + 1 && snapshot.html.includes(headingChip)
          ? snapshot
          : undefined,
      20_000,
    );
    assert.ok(
      afterCommand.html.includes(headingChip),
      `expected the right-click command to add a plan selection chip with source line (${headingChip})`,
    );

    // Selection → chat, path 2: the floating button on a different selection
    // (body paragraph) yields a second, distinct chip carrying its own line.
    await api.__testing.openPlanPreview(planPath);
    await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.bodyHasContent);
    await api.__testing.dispatchPlanPreviewDomAction(planPath, {
      kind: "selectText",
      selector: ".tc-plan-preview__body p",
    });
    await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.selectionButtonVisible);
    await api.__testing.dispatchPlanPreviewDomAction(planPath, { kind: "clickSelectionAdd" });
    const afterFloating = await waitForWebviewDomSnapshot(
      api,
      (snapshot) =>
        chipCount(snapshot.html) === baseChips + 2 && snapshot.html.includes(paragraphChip)
          ? snapshot
          : undefined,
      20_000,
    );
    assert.ok(
      afterFloating.html.includes(paragraphChip),
      `expected the floating button to add a plan selection chip with source line (${paragraphChip})`,
    );

    // Selection → chat, path 3 (P0 regression): two DIFFERENT line-less
    // selections from the same plan (todo items carry no source line) must BOTH
    // land as distinct chips. Before the dedupe fix they collapsed to a single
    // `selection::<path>::::` identity and every selection after the first was
    // silently dropped — the "add to chat 总是失败" the user reported.
    for (const [index, expectedChips] of [
      [1, baseChips + 3],
      [2, baseChips + 4],
    ] as const) {
      await api.__testing.openPlanPreview(planPath);
      await waitForPlanPreviewDom(api, planPath, (snapshot) => snapshot.bodyHasContent);
      await api.__testing.dispatchPlanPreviewDomAction(planPath, {
        kind: "selectText",
        selector: `.tc-plan-todos li:nth-child(${index}) .tc-plan-todo__content`,
      });
      await api.__testing.executeCommand(TOMCAT_PLAN_ADD_SELECTION_TO_CHAT_COMMAND);
      await waitForWebviewDomSnapshot(
        api,
        (snapshot) => (chipCount(snapshot.html) === expectedChips ? snapshot : undefined),
        20_000,
      );
    }

    // A regression: switching to native hides the in-body strip.
    await vscode.workspace
      .getConfiguration("tomcat")
      .update("plan.toolbarStyle", "native", vscode.ConfigurationTarget.Global);
    const native = await waitForPlanPreviewDom(
      api,
      planPath,
      (snapshot) => snapshot.toolbarStyle === "native" && !snapshot.hasActionStrip,
    );
    assert.equal(
      native.hasActionStrip,
      false,
      "expected no in-body action strip in native toolbar style",
    );
  } finally {
    await api.__testing.focusWebview().catch(() => undefined);
    await api.__testing.waitForWebviewReady().catch(() => undefined);
    await clearComposerDraft(api, sessionId).catch(() => undefined);
    await vscode.workspace
      .getConfiguration("tomcat")
      .update("plan.toolbarStyle", undefined, vscode.ConfigurationTarget.Global);
    await vscode.workspace
      .getConfiguration("tomcat")
      .update("plan.buildModel", "", vscode.ConfigurationTarget.Global);
    await fs.rm(scratchDir, { force: true, recursive: true }).catch(() => undefined);
  }
}
