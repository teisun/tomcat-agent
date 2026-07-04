import {
  assertApprovalDiffFlow,
  assertApprovalDiffFlowViaChatUi,
  assertInterruptAndRestartFlow,
  assertInterruptAndRestartFlowViaChatUi,
  assertModelSlashFlowViaChatUi,
  assertMultiSessionRouting,
  assertMultiSessionRoutingViaChatUi,
  assertParticipantHappyPath,
  assertParticipantHappyPathViaChatUi,
  assertPlanSlashFlowViaChatUi,
  assertWebviewPlanModeSwitchFlow,
  assertWebviewAnswerCardFlow,
  assertWebviewDiffFlow,
  assertWebviewCrossOwnerPlanFlow,
  assertWebviewFileDropReferenceFlow,
  assertWebviewPickContextFlow,
  assertWebviewGiantGroupLazyLoadFlow,
  assertWebviewInterruptFlow,
  assertWebviewMultiSessionFlow,
  assertWebviewOwnershipFlow,
  assertWebviewReloadReplayFlow,
  assertWebviewSelectionReferenceFlow,
  assertWebviewSessionSwitchRestoreFlow,
  assertWebviewStreamingFlow,
  assertTranscriptSwitchBackOrder,
  getTomcatExtensionApi,
} from "./support/hostE2eScenario";

suite("Tomcat host E2E", () => {
  test("runs the participant happy path", async () => {
    const api = await getTomcatExtensionApi();
    await assertParticipantHappyPath(api);
  });

  test("handles approval and diff/apply in a real host", async () => {
    const api = await getTomcatExtensionApi();
    await assertApprovalDiffFlow(api);
  });

  test("supports interrupt and restart in a real host", async () => {
    const api = await getTomcatExtensionApi();
    await assertInterruptAndRestartFlow(api);
  });

  test("keeps chat-thread to session routing stable in a real host", async () => {
    const api = await getTomcatExtensionApi();
    await assertMultiSessionRouting(api);
  });

  test("runs the participant happy path via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertParticipantHappyPathViaChatUi(api);
  });

  test("handles approval and diff/apply via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertApprovalDiffFlowViaChatUi(api);
  });

  test("supports interrupt and restart via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertInterruptAndRestartFlowViaChatUi(api);
  });

  test("keeps chat-thread routing stable via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertMultiSessionRoutingViaChatUi(api);
  });

  test("runs /plan via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertPlanSlashFlowViaChatUi(api);
  });

  test("switches an executing plan back to chat in the webview", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewPlanModeSwitchFlow(api);
  });

  test("runs /model via the real chat UI", async () => {
    const api = await getTomcatExtensionApi();
    await assertModelSlashFlowViaChatUi(api);
  });

  test("streams in the Tomcat webview", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewStreamingFlow(api);
  });

  test("applies edits from the Tomcat webview", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewDiffFlow(api);
  });

  test("renders ask_question answers in the Tomcat webview transcript", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewAnswerCardFlow(api);
  });

  test("resets interrupted Tomcat webview sessions back to send mode", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewInterruptFlow(api);
  });

  test("keeps multiple Tomcat webview sessions isolated", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewMultiSessionFlow(api);
  });

  test("enforces single-owner Tomcat webview sessions", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewOwnershipFlow(api);
  });

  test("restores plan cards and Ctx after switching sessions", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewSessionSwitchRestoreFlow(api);
  });

  test("replays plan history after a webview reload", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewReloadReplayFlow(api);
  });

  test("keeps transcript thinking and tool order stable after switching away and back", async () => {
    const api = await getTomcatExtensionApi();
    await assertTranscriptSwitchBackOrder(api);
  });

  test("lazy loads a giant historical tool group without rendering half a group", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewGiantGroupLazyLoadFlow(api);
  });

  test("keeps cross-owner plan state in sync in the webview", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewCrossOwnerPlanFlow(api);
  });

  test("adds editor selections to the webview composer and rehydrates them from history", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewSelectionReferenceFlow(api);
  });

  test("deduplicates dropped file references in the webview composer", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewFileDropReferenceFlow(api);
  });

  test("routes smart picker selections into attachments and context chips", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewPickContextFlow(api);
  });
});
