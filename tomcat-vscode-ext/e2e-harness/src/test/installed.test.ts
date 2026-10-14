import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as vscode from "vscode";

const repoRoot = path.resolve(__dirname, "../../../");
type HostE2eHelper = {
  assertSessionSettingsLanguageFlow(api: unknown): Promise<void>;
  assertMessageQueueDocksFlow(api: unknown): Promise<void>;
  assertPlanPreviewTablesFlow(api: unknown): Promise<void>;
  assertSessionFilesDockFlow(api: unknown): Promise<void>;
  assertWebviewPlanModeSwitchFlow(api: unknown): Promise<void>;
  assertWebviewAnswerCardFlow(api: unknown): Promise<void>;
  assertWebviewQuestionDisconnectFlow(api: unknown): Promise<void>;
  assertTranscriptUiFlow(api: unknown): Promise<void>;
  assertTranscriptSwitchBackOrder(api: unknown): Promise<void>;
  assertWebviewAddModelsFlow(api: unknown): Promise<void>;
  assertWebviewDiffFlow(api: unknown): Promise<void>;
  assertWebviewDraftForkPreservesReferenceFlow(api: unknown): Promise<void>;
  assertWebviewFileDropReferenceFlow(api: unknown): Promise<void>;
  assertWebviewPickContextFlow(api: unknown): Promise<void>;
  assertWebviewGiantGroupLazyLoadFlow(api: unknown): Promise<void>;
  assertWebviewInterruptFlow(api: unknown): Promise<void>;
  assertWebviewMultiSessionFlow(api: unknown): Promise<void>;
  assertWebviewReloadReplayFlow(api: unknown): Promise<void>;
  assertWebviewSelectionReferenceFlow(api: unknown): Promise<void>;
  assertWebviewSessionSwitchRestoreFlow(api: unknown): Promise<void>;
  assertWebviewStreamingFlow(api: unknown): Promise<void>;
  getTomcatExtensionApi(): Promise<unknown>;
};
type ResolvedSourceApi = {
  __testing: {
    getPromptHistory(): Array<{
      actions: string[];
      message: string;
      severity: string;
    }>;
    getResolvedExecutable(): {
      source: string;
    };
    getWebviewState(): {
      connectionStatus?: string;
      ready: boolean;
    };
  };
};

type PackagedWebviewApi = {
  __testing: {
    captureWebviewDom(): Promise<{ html: string }>;
    clearObservedWebviewErrors(): void;
    focusWebview(): Promise<void>;
    getObservedWebviewErrors(): Array<{ message: string; stack?: string }>;
    sendWebviewHostEvent(content: WebviewErrorBoundaryCrashFixture): Promise<void>;
    waitForWebviewReady(timeoutMs?: number): Promise<void>;
  };
};

type WebviewErrorBoundaryCrashFixture = {
  enabled: true;
  type: "__test.webview_error_boundary_crash";
};

type PromptEntry = ResolvedSourceApi["__testing"] extends {
  getPromptHistory(): Array<infer T>;
}
  ? T
  : never;

const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as HostE2eHelper;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

const workbenchCapture = require(path.resolve(repoRoot, "out/test/suite/support/workbenchFindDriver.js")) as {
  captureWorkbenchArtifacts(target: string): Promise<void>;
  SettingsFrameDriver: { connectFromEnvironment(selector: string): Promise<{
    click(testId: string): Promise<void>;
    capture(file: string): Promise<unknown>;
    waitForSnapshot(predicate: (snapshot: { buttons: Array<{ testId: string | null; disabled: boolean }> }) => boolean, description: string): Promise<unknown>;
    close(): void;
  }> };
  WorkbenchFindDriver: { connectFromEnvironment(): Promise<{
    waitForVisibleText(text: string): Promise<void>;
    clickVisibleButton(label: string): Promise<void>;
    close(): void;
  }> };
};

async function captureStartupCheckpoint(name: string, visibleText?: string): Promise<void> {
  if (process.env.TOMCAT_E2E_SCREENSHOT !== "1") return;
  const root = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  assert.ok(root, "this test launch must own an evidence directory");
  if (visibleText) {
    const driver = await workbenchCapture.WorkbenchFindDriver.connectFromEnvironment();
    try { await driver.waitForVisibleText(visibleText); } finally { driver.close(); }
  }
  await sleep(250);
  await workbenchCapture.captureWorkbenchArtifacts(path.join(root, `${name}.png`));
}

async function maybeTriggerOnboardingBootstrap(): Promise<void> {
  if (process.env.TOMCAT_EXPECT_PROMPT_TRIGGER !== "restart") {
    return;
  }
  try {
    await vscode.commands.executeCommand("tomcat.restartServe");
  } catch {
    // Setup-required scenarios intentionally fail the first initialize attempt.
  }
}

async function waitForPrompt(
  api: ResolvedSourceApi,
  expectedSubstring: string,
  timeoutMs: number,
): Promise<PromptEntry> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const prompt = api.__testing.getPromptHistory().find((entry) =>
      entry.message.includes(expectedSubstring),
    );
    if (prompt) {
      return prompt;
    }
    await sleep(100);
  }
  assert.fail(`Timed out waiting for prompt containing: ${expectedSubstring}`);
}

async function waitForServeReady(
  api: ResolvedSourceApi,
  timeoutMs = 15_000,
): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (api.__testing.getWebviewState().ready) {
      return;
    }
    await sleep(100);
  }
  assert.fail(
    `Timed out waiting for serve to become ready: ${JSON.stringify(api.__testing.getWebviewState())}`,
  );
}

async function waitForPackagedDom(
  api: PackagedWebviewApi,
  predicate: (snapshot: { html: string }) => boolean,
  timeoutMs = 10_000,
): Promise<{ html: string }> {
  const deadline = Date.now() + timeoutMs;
  let lastSnapshot = await api.__testing.captureWebviewDom();
  while (Date.now() < deadline) {
    if (predicate(lastSnapshot)) {
      return lastSnapshot;
    }
    await sleep(100);
    lastSnapshot = await api.__testing.captureWebviewDom();
  }
  assert.fail(`Timed out waiting for packaged webview DOM: ${lastSnapshot.html}`);
}

suite("Installed Tomcat extension", () => {
  test("session pins and language: installed bilingual controls and fixed review choices", async function () {
    this.timeout(180_000);
    await hostE2e.assertSessionSettingsLanguageFlow(await hostE2e.getTomcatExtensionApi());
  });
  test("uses the expected executable source when the host fixture asks for it", async function () {
    const expectedSource = process.env.TOMCAT_EXPECT_RESOLVED_SOURCE;
    if (!expectedSource) {
      this.skip();
      return;
    }

    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    assert.equal(api.__testing.getResolvedExecutable().source, expectedSource);
  });

  test("shows the expected onboarding prompt when requested by the host fixture", async function () {
    const expectedSubstring = process.env.TOMCAT_EXPECT_PROMPT_SUBSTRING;
    if (!expectedSubstring) {
      this.skip();
      return;
    }

    const expectedSeverity = process.env.TOMCAT_EXPECT_PROMPT_SEVERITY;
    const expectedActions = (process.env.TOMCAT_EXPECT_PROMPT_ACTIONS ?? "")
      .split("|")
      .map((value) => value.trim())
      .filter(Boolean);
    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    // Startup failures notify only while the chat panel is hidden.
    if (process.env.TOMCAT_EXPECT_PROMPT_TRIGGER === "restart") {
      await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar");
    }
    await maybeTriggerOnboardingBootstrap();
    const prompt = await waitForPrompt(api, expectedSubstring, 15_000);
    const expectedDetail = process.env.TOMCAT_EXPECT_PROMPT_DETAIL;
    if (expectedSeverity) {
      assert.equal(prompt.severity, expectedSeverity);
    }
    if (expectedActions.length > 0) {
      assert.deepEqual(prompt.actions, expectedActions);
    }
    if (expectedDetail) {
      assert.ok(
        prompt.message.includes(expectedDetail),
        `expected onboarding prompt to include serve stderr: ${expectedDetail}`,
      );
    }
    await captureStartupCheckpoint("onboarding-prompt", expectedSubstring);
  });

  test("recovers inline from a repaired startup failure without running init", async function () {
    if (process.env.TOMCAT_EXPECT_SETUP_RECOVERY !== "1") {
      this.skip();
      return;
    }
    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await vscode.commands.executeCommand("notifications.clearAll");
    await (api as unknown as PackagedWebviewApi).__testing.focusWebview();
    const promptsBefore = api.__testing.getPromptHistory().length;
    // Fails again because the fixture is not repaired yet, this time with the panel visible.
    await maybeTriggerOnboardingBootstrap();
    const driver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
    try {
      await driver.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "retry-connection" && !button.disabled), "Failure must offer an inline retry");
      assert.deepEqual(api.__testing.getPromptHistory().slice(promptsBefore), [], "A visible panel must report the failure inline instead of notifying");
      assert.equal(vscode.window.terminals.length, 0, "Failure must not launch an init terminal");
      const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
      if (artifacts) await driver.capture(path.join(artifacts, "startup-failed-inline.png"));
      // Repair only the fake startup precondition, not through the removed init workflow.
      const marker = process.env.TOMCAT_VSCODE_TEST_SETUP_MARKER;
      assert.ok(marker);
      await fs.mkdir(path.dirname(marker), { recursive: true });
      await fs.writeFile(marker, "fixture repaired\n");
      await driver.click("retry-connection");
      await waitForServeReady(api);
      await hostE2e.assertWebviewStreamingFlow(api as unknown);
      if (artifacts) {
        // Successful recovery can replace the webview execution context.
        const recovered = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
        try { await recovered.capture(path.join(artifacts, "startup-recovered-inline.png")); }
        finally { recovered.close(); }
      }
    } finally { driver.close(); }
  });

  test("self-heals one transient serve startup failure without showing setup", async function () {
    if (process.env.TOMCAT_EXPECT_TRANSIENT_SERVE_RECOVERY !== "1") {
      this.skip();
      return;
    }

    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await (api as unknown as PackagedWebviewApi).__testing.focusWebview();
    await (api as unknown as PackagedWebviewApi).__testing.waitForWebviewReady();
    await waitForServeReady(api);

    assert.equal(api.__testing.getWebviewState().connectionStatus, "ready");
    assert.ok(
      !api.__testing
        .getPromptHistory()
        .some((entry) => entry.message.includes("Tomcat failed to start")),
      "a transient startup failure must recover silently instead of suggesting setup",
    );
    await captureStartupCheckpoint("transient-startup-recovered");
  });

  test("connects when serve takes longer than the former handshake budget", async function () {
    if (process.env.TOMCAT_EXPECT_SLOW_HANDSHAKE !== "1") {
      this.skip();
      return;
    }

    this.timeout(60_000);
    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await (api as unknown as PackagedWebviewApi).__testing.focusWebview();
    await (api as unknown as PackagedWebviewApi).__testing.waitForWebviewReady(25_000);
    await waitForServeReady(api, 25_000);

    assert.equal(api.__testing.getWebviewState().connectionStatus, "ready");
    assert.ok(
      !api.__testing
        .getPromptHistory()
        .some((entry) => entry.message.includes("Tomcat failed to start")),
      "a slow but healthy handshake must not be treated as a startup crash",
    );
    if (process.env.TOMCAT_E2E_SCREENSHOT === "1") {
      // focusWebview above already awaits bootstrap. Capture a genuinely pending
      // connection, not an already-ready frame named "connecting".
      const restarting = Promise.resolve(vscode.commands.executeCommand("tomcat.restartServe"));
      restarting.catch(() => undefined);
      await waitForPackagedDom(api as unknown as PackagedWebviewApi,
        snapshot => snapshot.html.includes('data-testid="loading-state"') && snapshot.html.includes("tc-spinner"));
      const driver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
      try { await driver.capture(path.join(process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR!, "slow-handshake-reconnecting.png")); }
      finally { driver.close(); }
      await restarting;
      await waitForServeReady(api, 25_000);
    }
    await captureStartupCheckpoint("slow-handshake-ready");
  });

  test("message queue docks in the installed webview", async function () {
    this.timeout(180_000);
    await hostE2e.assertMessageQueueDocksFlow(await hostE2e.getTomcatExtensionApi());
  });
  test("plan preview tables in the installed webview", async function () {
    this.timeout(120_000);
    await hostE2e.assertPlanPreviewTablesFlow(await hostE2e.getTomcatExtensionApi());
  });
  test("session files dock in the installed webview", async function () {
    this.timeout(90_000);
    await hostE2e.assertSessionFilesDockFlow(await hostE2e.getTomcatExtensionApi());
  });

  test("switches an executing plan back to chat in the webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewPlanModeSwitchFlow(api);
  });

  test("packaged_webview_not_blank", async () => {
    const api = await hostE2e.getTomcatExtensionApi() as PackagedWebviewApi;
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    const snapshot = await api.__testing.captureWebviewDom();

    assert.ok(snapshot.html.trim().length > 0, "packaged webview rendered no DOM");
    assert.match(snapshot.html, /data-testid="stream-container"/u);
    assert.ok(
      !snapshot.html.includes('data-testid="webview-error-fallback"'),
      `packaged webview rendered the crash fallback: ${snapshot.html}`,
    );
  });

  test("streams in the Tomcat webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewStreamingFlow(api);
  });

  test("applies edits from the Tomcat webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewDiffFlow(api);
  });

  test("renders ask_question answers in the Tomcat webview transcript", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewAnswerCardFlow(api);
  });

  test("hydrates Disconnected after an irrecoverable serve/extension-host restart", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewQuestionDisconnectFlow(api);
  });

  test("resets interrupted Tomcat webview sessions back to send mode", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewInterruptFlow(api);
  });

  test("keeps multiple Tomcat webview sessions isolated", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewMultiSessionFlow(api);
  });

  test("restores plan cards and Ctx after switching sessions", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewSessionSwitchRestoreFlow(api);
  });

  test("replays plan history after a webview reload", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewReloadReplayFlow(api);
  });

  test("keeps transcript thinking and tool order stable after switching away and back", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertTranscriptSwitchBackOrder(api);
  });

  test("lazy loads a giant historical tool group without rendering half a group", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewGiantGroupLazyLoadFlow(api);
  });

  test("adds editor selections to the webview composer and rehydrates them from history", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewSelectionReferenceFlow(api);
  });

  test("deduplicates dropped file references in the webview composer", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewFileDropReferenceFlow(api);
  });

  test("keeps source draft references when creating a session", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewDraftForkPreservesReferenceFlow(api);
  });

  test("routes smart picker selections into attachments and context chips", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewPickContextFlow(api);
  });

  test("opens model settings and adds a model from the webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewAddModelsFlow(api);
  });

  test("renders the transcript UI groups, tool rows, file chips, and progress", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertTranscriptUiFlow(api);
  });

  // This intentionally remains last: the fixture replaces the live application with
  // ErrorBoundary's fallback for the rest of the webview document lifetime.
  test("packaged_webview_error_boundary_shows_fallback_and_reports_host", async () => {
    const api = await hostE2e.getTomcatExtensionApi() as PackagedWebviewApi;
    const expectedError = "E2E fixture intentionally crashed the Tomcat webview";
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    api.__testing.clearObservedWebviewErrors();
    await api.__testing.sendWebviewHostEvent({
      enabled: true,
      type: "__test.webview_error_boundary_crash",
    });

    const fallback = await waitForPackagedDom(
      api,
      (snapshot) =>
        snapshot.html.includes('data-testid="webview-error-fallback"') &&
        snapshot.html.includes(expectedError),
    );
    assert.match(fallback.html, /data-testid="webview-error-fallback"/u);
    assert.ok(
      api.__testing
        .getObservedWebviewErrors()
        .some((error) => error.message === expectedError),
      "expected the host to receive the webview error postMessage",
    );
  });
});
