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
  assertWebviewDiffFlow,
  assertWebviewMultiSessionFlow,
  assertWebviewOwnershipFlow,
  assertWebviewStreamingFlow,
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

  test("keeps multiple Tomcat webview sessions isolated", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewMultiSessionFlow(api);
  });

  test("enforces single-owner Tomcat webview sessions", async () => {
    const api = await getTomcatExtensionApi();
    await assertWebviewOwnershipFlow(api);
  });
});
