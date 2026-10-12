import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import {
  hostE2e, handleInitialProjectPrompt, pause, repoRoot, requiredEnv, waitFor, workbenchCapture,
  type Connector, type InstalledConnectorApi, type SettingsDriver,
} from "./connectors-installed-acceptance.test";

const selfTest = process.env.TOMCAT_CONNECTORS_ACCEPT_CHECKER_SELFTEST === "1";
const click = (api: InstalledConnectorApi, testId: string) => api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId });
const input = (api: InstalledConnectorApi, testId: string, value: string) => api.__testing.sendSettingsDomAction({ kind: "setInputValue", testId, value });
const row = (api: InstalledConnectorApi, name: string) => api.__testing.getSettingsPanelState().state.connectors?.find((source) => source.name === name);

async function openSettings(api: InstalledConnectorApi): Promise<SettingsDriver> {
  const executable = api.__testing.getResolvedExecutable();
  assert.ok(executable.found);
  assert.equal(path.resolve(executable.executable), path.resolve(requiredEnv("TOMCAT_CONNECTORS_ACCEPT_BINARY")), "must use this build, not a bundled/stale/fake CLI");
  await api.__testing.executeCommand("tomcat.openSettings");
  await waitFor("Settings ready", () => { const state = api.__testing.getSettingsPanelState(); return state.visible && state.webviewReady ? state : undefined; });
  const driver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
  try {
    if (api.__testing.getSettingsPanelState().route !== "connectors") {
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "settings-nav-connectors" && !button.disabled && button.width > 0), "Settings navigation committed after opening");
      await click(api, "settings-nav-connectors");
    }
    await waitFor("Connectors route", () => api.__testing.getSettingsPanelState().route === "connectors" ? true : undefined);
    await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-add-open" && button.width > 0), "Connectors page rendered");
    return driver;
  } catch (error) { driver.close(); throw error; }
}
async function addSource(api: InstalledConnectorApi, name: string, transport: { url: string } | { args: string[] }): Promise<Connector> {
  if ("args" in transport) {
    // Setup goes through the real Host intent with an argv array. The stdio Add
    // form is covered separately; this avoids its whitespace-only args parser.
    await api.__testing.sendSettingsIntent({ type: "addConnector", messageId: `add-${name}`, data: { connector: { type: "mcp", name, transport: "stdio", command: "node", args: transport.args, scope: "workspace", env: {} } } });
  } else {
    await click(api, "connector-add-open");
    await input(api, "connector-name", name);
    await click(api, "connector-transport-http");
    await input(api, "connector-url", transport.url);
    await input(api, "connector-auth", "none");
    await click(api, "connector-scope-workspace");
    await pause(200); // let the real React form commit its new submit closure
    await click(api, "connector-add-submit");
  }
  const source = await waitFor(`${name} Connected`, () => { const found = row(api, name); return found?.state === "connected" ? found : undefined; });
  assert.equal(source.source, "workspace");
  return source;
}
async function openDetail(api: InstalledConnectorApi, driver: SettingsDriver, name: string): Promise<void> {
  await click(api, `connector-card-${name}`);
  await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && button.width > 0), "Connector detail rendered");
}
async function closeDetail(api: InstalledConnectorApi, driver: SettingsDriver): Promise<void> {
  await click(api, "connector-detail-done");
  await driver.waitForSnapshot((dom) => !dom.buttons.some((button) => button.testId === "connector-reload" && button.width > 0), "Connector detail closed");
}
async function control(value: Record<string, unknown>): Promise<void> {
  const response = await fetch(`${requiredEnv("TOMCAT_CONNECTORS_ACCEPT_MCP_URL")}/__test/control`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(value), signal: AbortSignal.timeout(5_000) });
  assert.equal(response.status, 204);
}
async function serverState(): Promise<{ initialized: number; calls: unknown[]; requests: unknown[] }> {
  const response = await fetch(`${requiredEnv("TOMCAT_CONNECTORS_ACCEPT_MCP_URL")}/__test/state`, { signal: AbortSignal.timeout(5_000) });
  assert.equal(response.status, 200);
  return response.json() as Promise<{ initialized: number; calls: unknown[]; requests: unknown[] }>;
}

suite("Installed Settings Reload and recovery evidence", () => {
  suiteSetup(async function () { this.timeout(80_000); await handleInitialProjectPrompt(); });
  (selfTest ? test : test.skip)("rejects a known error emitted in the actual Settings frame", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    const driver = await openSettings(api);
    const root = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    try {
      assert.deepEqual(api.__testing.getObservedWebviewErrors(), [], "the chat recorder is empty before the Settings-only probe");
      await driver.evaluate(`console.error('TOMCAT_SETTINGS_CHECKER_SELFTEST')`);
      await assert.rejects(driver.capture(path.join(root, "settings-checker-probe.png")), /TOMCAT_SETTINGS_CHECKER_SELFTEST/);
      const report = JSON.parse(await fs.readFile(path.join(root, "settings-checker-probe.console.json"), "utf8"));
      assert.ok(report.errors.some((event: unknown) => JSON.stringify(event).includes("TOMCAT_SETTINGS_CHECKER_SELFTEST")));
      await fs.writeFile(path.join(root, "checker-selftest.json"), JSON.stringify({ detectedExpectedSettingsError: true, chatErrors: api.__testing.getObservedWebviewErrors() }, null, 2));
    } finally { driver.close(); }
  });

  (process.env.TOMCAT_CONNECTORS_ACCEPT_PHASE === "normal" ? test : test.skip)("renders Reload progress, both outcomes, new tools, and an actual SDK-task recovery", async function () {
    this.timeout(180_000);
    const api = await hostE2e.getTomcatExtensionApi();
    const driver = await openSettings(api);
    const root = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    const httpName = "installed-http-reload";
    const stdioName = "installed-stdio-recovery";
    const data = path.join(root, "fixture-data");
    await fs.mkdir(data, { recursive: true });
    const startupGate = path.join(data, "stdio-ready");
    const exitGate = path.join(data, "stdio-exit");
    const once = path.join(data, "stdio-exited-once");
    const record = path.join(data, "stdio-methods.log");
    const sources: Connector[] = [];
    try {
      // The previous installation case leaves its OAuth details open.
      const initial = await driver.snapshot();
      if (initial.buttons.some((button) => button.testId === "connector-detail-done" && button.width > 0)) await closeDetail(api, driver);
      await driver.setViewport(1440, 900);
      const source = await addSource(api, httpName, { url: `${requiredEnv("TOMCAT_CONNECTORS_ACCEPT_MCP_URL")}/mcp` });
      sources.push(source);
      await openDetail(api, driver, httpName);
      await waitFor("initial tool catalog", () => api.__testing.getSettingsPanelState().state.connectorTools?.some((tool) => tool.rawName === "echo") ? true : undefined);
      const before = await serverState();
      await control({ initGate: "installed-reload-success", extraTool: "echo_after_reload" });
      // Two actual DOM events in one turn exercise the click-side single-flight guard.
      await driver.evaluate(`(() => { const button = document.querySelector('[data-testid="connector-reload"]'); button.click(); button.click(); })()`);
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && button.disabled && button.busy === "true"), "Reload is visibly busy");
      await driver.capture(path.join(root, "reload-pending-desktop.png"));
      await waitFor("correlated Reload accepted", () => api.__testing.getSettingsPanelState().state.connectorReloads?.[source.configKey]?.phase === "accepted" ? true : undefined);
      assert.equal((await serverState()).initialized, before.initialized + 1, "double-click sends only one initialize");
      await closeDetail(api, driver);
      await driver.capture(path.join(root, "reload-card-progress.png"));
      await openDetail(api, driver, httpName);
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && button.disabled), "reopening uses the backend recovery state");
      await driver.capture(path.join(root, "reload-reopened-progress.png"));
      await control({ release: "installed-reload-success", initGate: null });
      await waitFor("Reload success receipt", () => api.__testing.getSettingsPanelState().state.connectorReloads?.[source.configKey]?.phase === "succeeded" ? true : undefined);
      await waitFor("new tool identity and directory", () => {
        const state = api.__testing.getSettingsPanelState().state;
        const current = row(api, httpName);
        return current?.state === "connected" && state.connectorTools?.some((tool) => tool.rawName === "echo_after_reload")
          && state.connectorToolsIdentity?.generation === current.generation && state.connectorToolsIdentity?.attempt === current.attempt ? state : undefined;
      });
      await driver.waitForSnapshot((dom) => dom.text.includes("echo_after_reload") && dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "success renders new tools and releases busy");
      await driver.capture(path.join(root, "reload-success-desktop.png"));

      const successCount = (await serverState()).initialized;
      await control({ initFailures: 3, initStatus: 503 });
      await click(api, "connector-reload");
      await waitFor("bounded Reload failure", () => api.__testing.getSettingsPanelState().state.connectorReloads?.[source.configKey]?.phase === "failed" ? true : undefined);
      assert.equal(row(api, httpName)?.state, "failed");
      assert.equal((await serverState()).initialized, successCount + 3, "failed recovery stops at its three-attempt budget");
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "failure releases busy");
      await driver.capture(path.join(root, "reload-failed-desktop.png"));
      await api.__testing.executeCommand("workbench.action.closeSidebar");
      await api.__testing.executeCommand("workbench.action.closeAuxiliaryBar");
      await driver.setViewport(430, 844);
      await driver.waitForSnapshot((dom) => dom.viewport.width <= 430 && dom.viewport.width >= 250, "narrow Settings frame reflow");
      const narrow = await driver.capture(path.join(root, "reload-failed-narrow.png"));
      assert.ok(narrow.viewport.width <= 430 && narrow.viewport.width >= 250, `actual narrow Settings viewport: ${JSON.stringify(narrow.viewport)}`);
      assert.ok(narrow.feedback.length > 0, "failure feedback is rendered");
      assert.ok(narrow.feedback.every((feedback) => feedback.scrollWidth <= feedback.width + 1), "long errors wrap instead of overflowing");
      await driver.focusAndPress("connector-detail-done", "Enter");
      await driver.waitForSnapshot((dom) => !dom.buttons.some((button) => button.testId === "connector-reload" && button.width > 0), "Done works from the keyboard");
      const narrowList = await driver.capture(path.join(root, "reload-failed-cards-narrow.png"));
      const cards = narrowList.buttons.filter((button) => button.testId?.startsWith("connector-card-"));
      assert.ok(cards.length > 0 && cards.every((card) => card.width >= narrowList.viewport.width * 0.65 && card.height < 250), "narrow cards remain readable rather than one character per line");
      await driver.setViewport(1440, 900);

      // A stdio process really exits once. The next process waits at initialize,
      // giving the UI a deterministic automatic-recovery state without fake frames.
      await fs.writeFile(startupGate, "ready");
      const fixture = path.resolve(repoRoot, "../tomcat/tests/fixtures/mcp/fake_stdio_server.mjs");
      const startupFailures = path.join(data, "fail-startup");
      const args = [fixture, "--startup-gate-file", startupGate, "--exit-gate-file", exitGate, "--exit-once-file", once, "--record", record, "--fail-startup-file", startupFailures];
      const recovering = await addSource(api, stdioName, { args });
      sources.push(recovering);
      await fs.unlink(startupGate);
      await fs.writeFile(exitGate, "exit now");
      await waitFor("SDK background-task exit starts automatic recovery", () => { const current = row(api, stdioName); return current?.recovery && current.attempt === 2 ? current : undefined; });
      await openDetail(api, driver, stdioName);
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && button.disabled), "automatic recovery is busy without a click receipt");
      assert.equal(api.__testing.getSettingsPanelState().state.connectorReloads?.[recovering.configKey], undefined);
      await driver.capture(path.join(root, "automatic-recovery-progress.png"));
      await fs.writeFile(startupGate, "ready again");
      await waitFor("automatic recovery Connected", () => { const current = row(api, stdioName); return current?.state === "connected" && current.attempt === 2 ? current : undefined; });
      await driver.waitForSnapshot((dom) => dom.text.includes("Tools (2)") && dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "automatic recovery terminal state and new directory rendered");
      await driver.capture(path.join(root, "automatic-recovery-connected.png"));
      const methods = (await fs.readFile(record, "utf8")).trim().split("\n");
      assert.equal(methods.filter((method) => method === "initialize").length, 2);
      assert.equal(methods.filter((method) => method === "process-exit").length, 1);
      assert.equal(methods.filter((method) => method === "tools/call").length, 0, "restoring the connection must not invent or replay a call");
      // The same source then fails every new handshake. Only explicit Reload
      // grants a new budget, and subsequent Settings polling must not add attempts.
      await fs.writeFile(startupFailures, "close initialize");
      await driver.focusAndPress("connector-reload", "Enter");
      await waitFor("stdio recovery budget exhausted", () => api.__testing.getSettingsPanelState().state.connectorReloads?.[recovering.configKey]?.phase === "failed" ? true : undefined);
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "failed stdio recovery releases busy");
      await driver.capture(path.join(root, "automatic-source-budget-exhausted.png"));
      const stopped = await fs.readFile(record, "utf8");
      assert.equal(stopped.split("\n").filter((method) => method === "startup-exit").length, 3);
      await pause(5_500); // at least one ordinary 5-second Settings polling interval
      assert.equal(await fs.readFile(record, "utf8"), stopped, "polling does not restart an exhausted source");
      await fs.unlink(startupFailures);
      await click(api, "connector-reload");
      await waitFor("explicit Reload restores exhausted source", () => api.__testing.getSettingsPanelState().state.connectorReloads?.[recovering.configKey]?.phase === "succeeded" ? true : undefined);
      await driver.waitForSnapshot((dom) => dom.text.includes("Tools (2)") && dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "explicit recovery terminal state and new directory rendered");
      await driver.capture(path.join(root, "automatic-source-explicit-reload.png"));
      const finalMethods = (await fs.readFile(record, "utf8")).trim().split("\n");
      assert.equal(finalMethods.filter((method) => method === "initialize").length, 6);
      assert.equal(finalMethods.filter((method) => method === "tools/call").length, 0);
      await driver.focusAndPress("connector-remove", "Enter");
      await waitFor("keyboard Remove deletes the fixture source", () => !row(api, stdioName) ? true : undefined);
      await driver.waitForSnapshot((dom) => !dom.buttons.some((button) => button.testId === `connector-card-${stdioName}` || button.testId === "connector-reload"), "keyboard Remove updates the list and closes details");
      sources.pop(); // the stdio source was already removed through the real UI
      await driver.capture(path.join(root, "keyboard-remove-completed.png"));
      await api.__testing.sendSettingsIntent({ type: "settings.ready", messageId: "models-layout", data: { route: "models" } });
      await waitFor("Models data ready for shared responsive layout", () => api.__testing.getSettingsPanelState().state.models.length > 0 ? true : undefined);
      await driver.setViewport(430, 844);
      await driver.capture(path.join(root, "settings-models-narrow.png"));
      await driver.setViewport(1440, 900);
      await fs.writeFile(path.join(root, "reload-backend-evidence.json"), JSON.stringify({ http: await serverState(), stdioMethods: finalMethods, settings: api.__testing.getSettingsPanelState().state }, null, 2));
    } catch (error) {
      await fs.writeFile(path.join(root, "reload-failure-state.json"), JSON.stringify({ settings: api.__testing.getSettingsPanelState(), dom: await driver.snapshot(), http: await serverState() }, null, 2));
      throw error;
    } finally {
      // Release owned fixture gates even after a failed UI assertion.
      await Promise.all([fs.writeFile(startupGate, "cleanup"), fs.writeFile(exitGate, "cleanup")]);
      for (const source of sources) await api.__testing.sendSettingsIntent({ type: "removeConnector", messageId: `cleanup-${source.name}`, data: { name: source.name, configKey: source.configKey } }).catch(() => undefined);
      driver.close();
    }
  });
});
