import {
  assertApprovalDiffFlow,
  assertInterruptAndRestartFlow,
  assertMultiSessionRouting,
  assertParticipantHappyPath,
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
});
