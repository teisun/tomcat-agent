import {
  assertApprovalDiffFlow,
  assertApprovalDiffFlowViaChatUi,
  assertInterruptAndRestartFlow,
  assertInterruptAndRestartFlowViaChatUi,
  assertMultiSessionRouting,
  assertMultiSessionRoutingViaChatUi,
  assertParticipantHappyPath,
  assertParticipantHappyPathViaChatUi,
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
});
