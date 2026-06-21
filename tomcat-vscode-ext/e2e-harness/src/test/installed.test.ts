import * as path from "node:path";

const repoRoot = path.resolve(__dirname, "../../../");
type HostE2eHelper = {
  assertApprovalDiffFlow(api: unknown): Promise<void>;
  assertApprovalDiffFlowViaChatUi(api: unknown): Promise<void>;
  assertInterruptAndRestartFlow(api: unknown): Promise<void>;
  assertInterruptAndRestartFlowViaChatUi(api: unknown): Promise<void>;
  assertMultiSessionRouting(api: unknown): Promise<void>;
  assertMultiSessionRoutingViaChatUi(api: unknown): Promise<void>;
  assertParticipantHappyPath(api: unknown): Promise<void>;
  assertParticipantHappyPathViaChatUi(api: unknown): Promise<void>;
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
});
