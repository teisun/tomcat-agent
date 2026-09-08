import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

const repoRoot = path.resolve(__dirname, "../../../");
type HostE2eHelper = {
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

  test("recovers from a setup-required startup when the test fixture auto-runs init", async function () {
    if (process.env.TOMCAT_EXPECT_SETUP_RECOVERY !== "1") {
      this.skip();
      return;
    }

    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await maybeTriggerOnboardingBootstrap();
    await waitForPrompt(api, "Tomcat could not start after several attempts.", 20_000);
    await captureStartupCheckpoint("setup-required-before-recovery", "Tomcat could not start after several attempts.");
    if (process.env.TOMCAT_EXPECT_VISIBLE_SETUP === "1") {
      const driver = await workbenchCapture.WorkbenchFindDriver.connectFromEnvironment();
      try { await driver.clickVisibleButton("Start Setup"); } finally { driver.close(); }
    }

    const deadline = Date.now() + 20_000;
    let lastError: unknown;
    while (Date.now() < deadline) {
      try {
        await hostE2e.assertWebviewStreamingFlow(api as unknown);
        await captureStartupCheckpoint("setup-recovered-stream");
        return;
      } catch (error) {
        lastError = error;
        await sleep(1_000);
      }
    }

    throw lastError instanceof Error
      ? lastError
      : new Error("Timed out waiting for setup-required recovery to succeed");
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
        .some((entry) => entry.message.includes("Tomcat could not start after several attempts.")),
      "a transient startup failure must recover silently instead of suggesting setup",
    );
    await captureStartupCheckpoint("transient-startup-recovered");
  });

  test("connects when serve takes longer than the former handshake budget", async function () {
    if (process.env.TOMCAT_EXPECT_SLOW_HANDSHAKE !== "1") {
      this.skip();
      return;
    }

    this.timeout(35_000);
    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await (api as unknown as PackagedWebviewApi).__testing.focusWebview();
    await (api as unknown as PackagedWebviewApi).__testing.waitForWebviewReady(25_000);
    await waitForServeReady(api, 25_000);

    assert.equal(api.__testing.getWebviewState().connectionStatus, "ready");
    assert.ok(
      !api.__testing
        .getPromptHistory()
        .some((entry) => entry.message.includes("Tomcat could not start after several attempts.")),
      "a slow but healthy handshake must not be treated as a startup crash",
    );
    await captureStartupCheckpoint("slow-handshake-ready");
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
