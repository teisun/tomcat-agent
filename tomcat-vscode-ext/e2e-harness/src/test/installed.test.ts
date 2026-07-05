import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

const repoRoot = path.resolve(__dirname, "../../../");
type HostE2eHelper = {
  assertApprovalDiffFlow(api: unknown): Promise<void>;
  assertApprovalDiffFlowViaChatUi(api: unknown): Promise<void>;
  assertInterruptAndRestartFlow(api: unknown): Promise<void>;
  assertInterruptAndRestartFlowViaChatUi(api: unknown): Promise<void>;
  assertModelSlashFlowViaChatUi(api: unknown): Promise<void>;
  assertMultiSessionRouting(api: unknown): Promise<void>;
  assertMultiSessionRoutingViaChatUi(api: unknown): Promise<void>;
  assertParticipantHappyPath(api: unknown): Promise<void>;
  assertParticipantHappyPathViaChatUi(api: unknown): Promise<void>;
  assertPlanSlashFlowViaChatUi(api: unknown): Promise<void>;
  assertWebviewPlanModeSwitchFlow(api: unknown): Promise<void>;
  assertWebviewAnswerCardFlow(api: unknown): Promise<void>;
  assertTranscriptUiFlow(api: unknown): Promise<void>;
  assertTranscriptSwitchBackOrder(api: unknown): Promise<void>;
  assertWebviewCrossOwnerPlanFlow(api: unknown): Promise<void>;
  assertWebviewDiffFlow(api: unknown): Promise<void>;
  assertWebviewFileDropReferenceFlow(api: unknown): Promise<void>;
  assertWebviewPickContextFlow(api: unknown): Promise<void>;
  assertWebviewGiantGroupLazyLoadFlow(api: unknown): Promise<void>;
  assertWebviewInterruptFlow(api: unknown): Promise<void>;
  assertWebviewMultiSessionFlow(api: unknown): Promise<void>;
  assertWebviewOwnershipFlow(api: unknown): Promise<void>;
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
  };
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
    if (expectedSeverity) {
      assert.equal(prompt.severity, expectedSeverity);
    }
    if (expectedActions.length > 0) {
      assert.deepEqual(prompt.actions, expectedActions);
    }
  });

  test("recovers from a setup-required startup when the test fixture auto-runs init", async function () {
    if (process.env.TOMCAT_EXPECT_SETUP_RECOVERY !== "1") {
      this.skip();
      return;
    }

    const api = await hostE2e.getTomcatExtensionApi() as ResolvedSourceApi;
    await maybeTriggerOnboardingBootstrap();
    await waitForPrompt(api, "Tomcat is installed, but it is not ready yet", 20_000);

    const deadline = Date.now() + 20_000;
    let lastError: unknown;
    while (Date.now() < deadline) {
      try {
        await hostE2e.assertParticipantHappyPath(api as unknown);
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

  test("runs the participant happy path", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertParticipantHappyPath(api);
  });

  test("handles approval and diff/apply in a real host", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertApprovalDiffFlow(api);
  });

  test("supports interrupt and restart in a real host", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertInterruptAndRestartFlow(api);
  });

  test("keeps chat-thread to session routing stable in a real host", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertMultiSessionRouting(api);
  });

  test("runs the participant happy path via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertParticipantHappyPathViaChatUi(api);
  });

  test("handles approval and diff/apply via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertApprovalDiffFlowViaChatUi(api);
  });

  test("supports interrupt and restart via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertInterruptAndRestartFlowViaChatUi(api);
  });

  test("keeps chat-thread routing stable via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertMultiSessionRoutingViaChatUi(api);
  });

  test("runs /plan via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertPlanSlashFlowViaChatUi(api);
  });

  test("switches an executing plan back to chat in the webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewPlanModeSwitchFlow(api);
  });

  test("runs /model via the real chat UI", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertModelSlashFlowViaChatUi(api);
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

  test("resets interrupted Tomcat webview sessions back to send mode", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewInterruptFlow(api);
  });

  test("keeps multiple Tomcat webview sessions isolated", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewMultiSessionFlow(api);
  });

  test("enforces single-owner Tomcat webview sessions", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewOwnershipFlow(api);
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

  test("keeps cross-owner plan state in sync in the webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewCrossOwnerPlanFlow(api);
  });

  test("adds editor selections to the webview composer and rehydrates them from history", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewSelectionReferenceFlow(api);
  });

  test("deduplicates dropped file references in the webview composer", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewFileDropReferenceFlow(api);
  });

  test("routes smart picker selections into attachments and context chips", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewPickContextFlow(api);
  });

  test("renders the transcript UI groups, tool rows, file chips, and progress", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertTranscriptUiFlow(api);
  });
});
