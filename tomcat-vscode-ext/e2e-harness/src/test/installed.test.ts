import * as path from "node:path";

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
  assertWebviewAnswerCardFlow(api: unknown): Promise<void>;
  assertTranscriptUiFlow(api: unknown): Promise<void>;
  assertWebviewCrossOwnerPlanFlow(api: unknown): Promise<void>;
  assertWebviewDiffFlow(api: unknown): Promise<void>;
  assertWebviewGiantGroupLazyLoadFlow(api: unknown): Promise<void>;
  assertWebviewInterruptFlow(api: unknown): Promise<void>;
  assertWebviewMultiSessionFlow(api: unknown): Promise<void>;
  assertWebviewOwnershipFlow(api: unknown): Promise<void>;
  assertWebviewReloadReplayFlow(api: unknown): Promise<void>;
  assertWebviewSessionSwitchRestoreFlow(api: unknown): Promise<void>;
  assertWebviewStreamingFlow(api: unknown): Promise<void>;
  getTomcatExtensionApi(): Promise<unknown>;
};

const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as HostE2eHelper;

suite("Installed Tomcat extension", () => {
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

  test("lazy loads a giant historical tool group without rendering half a group", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewGiantGroupLazyLoadFlow(api);
  });

  test("keeps cross-owner plan state in sync in the webview", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertWebviewCrossOwnerPlanFlow(api);
  });

  test("renders the transcript UI groups, tool rows, file chips, and progress", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    await hostE2e.assertTranscriptUiFlow(api);
  });
});
