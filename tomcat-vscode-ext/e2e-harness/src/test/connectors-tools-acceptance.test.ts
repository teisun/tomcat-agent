import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";

import {
  handleInitialProjectPrompt,
  hostE2e,
  pause,
  requiredEnv,
  waitFor,
  workbenchCapture,
} from "./connectors-installed-acceptance.test";

const connectorName = "installed-controlled-stdio";
const toolTestId = (rawName: string) => `connector-tool-${rawName}`;

async function openControlledConnector() {
  const api = await hostE2e.getTomcatExtensionApi();
  await api.__testing.executeCommand("tomcat.openSettings");
  await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "settings-nav-connectors" });
  await waitFor("connector settings route", () => {
    const panel = api.__testing.getSettingsPanelState();
    return panel.route === "connectors" && panel.visible && panel.webviewReady ? panel : undefined;
  });
  const connector = await waitFor("controlled stdio connector", () => api
    .__testing
    .getSettingsPanelState()
    .state
    .connectors
    ?.find((entry) => entry.name === connectorName && entry.state === "connected"));
  await api.__testing.sendSettingsDomAction({
    kind: "clickTestId",
    testId: `connector-card-${connectorName}`,
  });
  await waitFor("complete controlled tool catalog", () => {
    const state = api.__testing.getSettingsPanelState().state;
    const tools = state.connectorTools;
    return state.selectedConnector === connector.configKey
      && tools?.length === 2
      && tools.every((tool) => tool.rawName === "capture" || tool.rawName === "status")
      ? { connector, tools }
      : undefined;
  });
  return { api, connector };
}

suite("Installed real-Serve connector tool toggle acceptance", () => {
  suiteSetup(async function () { this.timeout(80_000); await handleInitialProjectPrompt(); });
  (process.env.TOMCAT_CONNECTORS_ACCEPT_PHASE === "normal" ? test : test.skip)("keeps disabled tools manageable and restores them without implicit reload", async () => {
    const { api, connector } = await openControlledConnector();
    const artifacts = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    const initialGeneration = connector.generation;
    const initialAttempt = connector.attempt;
    const settings = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    try {
      const normalStyle = await settings.evaluate<{
        background: string;
        color: string;
        textDecoration: string;
      }>(`(() => {
        const link = document.querySelector('[data-testid="connector-config-path"]');
        if (!link) throw new Error('connector configuration link is missing');
        const style = getComputedStyle(link);
        return { background: style.backgroundColor, color: style.color, textDecoration: style.textDecorationLine };
      })()`);
      assert.notEqual(normalStyle.color, "", "the rendered configuration link must have a themed color");
      assert.equal(normalStyle.textDecoration, "none", "the configuration link must not look permanently underlined");
      assert.match(normalStyle.background, /transparent|rgba\(0,\s*0,\s*0,\s*0\)/u, "the configuration link must not inherit code background styling");
      await settings.capture(path.join(artifacts, "connector-tools-path-normal.png"));
      await settings.hover("connector-config-path");
      const hoverDecoration = await settings.evaluate<string>(`getComputedStyle(document.querySelector('[data-testid="connector-config-path"]')).textDecorationLine`);
      assert.match(hoverDecoration, /underline/u, "hover must make the configuration link discoverable");
      await settings.capture(path.join(artifacts, "connector-tools-path-hover.png"));
      const focusedStyle = await settings.evaluate<{ focused: boolean; outline: string; textDecoration: string }>(`(() => {
        const link = document.querySelector('[data-testid="connector-config-path"]');
        link.focus();
        const style = getComputedStyle(link);
        return { focused: document.activeElement === link, outline: style.outlineStyle, textDecoration: style.textDecorationLine };
      })()`);
      assert.ok(focusedStyle.focused, "the configuration link must remain keyboard focusable");
      assert.match(focusedStyle.textDecoration, /underline/u, "keyboard focus must underline the configuration link");
      assert.notEqual(focusedStyle.outline, "none", "keyboard focus must have a visible outline");
      await settings.capture(path.join(artifacts, "connector-tools-path-focus.png"));

      await settings.focusAndPress(toolTestId("capture"), "Enter");
      const captureReceipt = await waitFor("capture toggle receipt", () => {
        const state = api.__testing.getSettingsPanelState().state;
        return Object.values(state.connectorToolToggles ?? {}).find((receipt) => receipt.rawName === "capture");
      });
      await fs.writeFile(
        path.join(artifacts, "connector-tools-after-capture-toggle.json"),
        JSON.stringify(api.__testing.getSettingsPanelState(), null, 2),
        "utf8",
      );
      assert.equal(captureReceipt.configSaved, true, `capture setting must be persisted: ${captureReceipt.error ?? "unknown error"}`);
      assert.equal(captureReceipt.runtimeApplied, true, `capture setting must be applied: ${captureReceipt.error ?? "unknown error"}`);
      const captureDisabled = await waitFor("capture disabled without removing its row", () => {
        const state = api.__testing.getSettingsPanelState().state;
        const tools = state.connectorTools;
        return tools?.length === 2 && tools.find((tool) => tool.rawName === "capture")?.enabled === false
          && tools.find((tool) => tool.rawName === "status")?.enabled === true
          ? state
          : undefined;
      });
      const afterCapture = captureDisabled.connectors?.find((entry) => entry.configKey === connector.configKey);
      assert.equal(afterCapture?.generation, initialGeneration, "a single toggle must not reconnect the source");
      assert.equal(afterCapture?.attempt, initialAttempt, "a single toggle must not start another connection attempt");
      await settings.waitForSnapshot(
        (snapshot) => snapshot.buttons.some((button) => button.testId === toolTestId("capture"))
          && snapshot.text.includes("Tools (2) · 1 enabled"),
        "the rendered disabled tool switch and count have committed",
      );
      const captureAriaChecked = await settings.evaluate<string | null>(`document.querySelector('[data-testid="connector-tool-capture"]')?.getAttribute("aria-checked") ?? null`);
      assert.equal(captureAriaChecked, "false", "the disabled tool stays as an unchecked switch");
      const captureAccessibleName = await settings.evaluate<string | null>(`document.querySelector('[data-testid="connector-tool-capture"]')?.getAttribute("aria-label") ?? null`);
      assert.equal(captureAccessibleName, "capture", "the switch name must remain stable while its state changes");
      const displayedToolCount = await settings.evaluate<string>(`document.querySelector(".tc-connector-tools-heading h3")?.textContent ?? ""`);
      assert.equal(displayedToolCount, "Tools (2) · 1 enabled", "the detail must distinguish total and enabled tool counts");
      await settings.capture(path.join(artifacts, "connector-tools-capture-disabled.png"));

      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: toolTestId("status") });
      await waitFor("both tools disabled but still listed", () => {
        const tools = api.__testing.getSettingsPanelState().state.connectorTools;
        return tools?.length === 2 && tools.every((tool) => tool.enabled === false) ? tools : undefined;
      });
      const allDisabledSnapshot = await settings.waitForSnapshot(
        (snapshot) => snapshot.text.includes("Tools (2) · 0 enabled"),
        "the rendered all-disabled tool count",
      );
      assert.ok(allDisabledSnapshot.text.includes("Tools (2) · 0 enabled"), "all disabled tools must remain visible and manageable");

      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-detail-done" });
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: `connector-card-${connectorName}` });
      await waitFor("disabled catalog after closing and reopening details", () => {
        const state = api.__testing.getSettingsPanelState().state;
        return state.connectorTools?.length === 2 && state.connectorTools.every((tool) => tool.enabled === false)
          ? state
          : undefined;
      });
      await settings.waitForSnapshot(
        (snapshot) => snapshot.buttons.some((button) => button.testId === "connector-reload")
          && snapshot.buttons.some((button) => button.testId === toolTestId("capture")),
        "reopened connector controls",
      );
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-reload" });
      await waitFor("explicit reload completes", () => api
        .__testing
        .getSettingsPanelState()
        .state
        .connectors
        ?.find((entry) => entry.configKey === connector.configKey && entry.state === "connected"));
      await waitFor("disabled catalog survives explicit reload", () => {
        const tools = api.__testing.getSettingsPanelState().state.connectorTools;
        return tools?.length === 2 && tools.every((tool) => tool.enabled === false) ? tools : undefined;
      });
      await settings.waitForSnapshot(
        (snapshot) => snapshot.buttons.some((button) => button.testId === toolTestId("capture")),
        "reloaded capture switch",
      );

      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: toolTestId("capture") });
      await waitFor("capture restored", () => api
        .__testing
        .getSettingsPanelState()
        .state
        .connectorTools
        ?.find((tool) => tool.rawName === "capture" && tool.enabled === true));
      await settings.waitForSnapshot(
        (snapshot) => snapshot.buttons.some((button) => button.testId === toolTestId("status")),
        "restored status switch",
      );
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: toolTestId("status") });
      await waitFor("all tools restored", () => {
        const tools = api.__testing.getSettingsPanelState().state.connectorTools;
        return tools?.length === 2 && tools.every((tool) => tool.enabled) ? tools : undefined;
      });
      await settings.capture(path.join(artifacts, "connector-tools-restored.png"));

      await settings.setViewport(390, 844);
      const narrow = await settings.capture(path.join(artifacts, "connector-tools-narrow.png"));
      const pathFits = await settings.evaluate<boolean>(`(() => {
        const link = document.querySelector('[data-testid="connector-config-path"]');
        return link && link.scrollWidth <= link.clientWidth;
      })()`);
      assert.ok(pathFits, "the configuration path must not horizontally overflow on a narrow Settings frame");
      assert.ok(narrow.viewport.width <= 390, "narrow capture must use the requested viewport");
      await fs.writeFile(path.join(artifacts, "connector-tools.dom.html"), (await api.__testing.captureSettingsDom()).html, "utf8");
      await fs.writeFile(path.join(artifacts, "connector-tools-console.json"), JSON.stringify(api.__testing.getObservedWebviewErrors(), null, 2), "utf8");
      assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    } finally {
      await settings.setViewport(1440, 900).catch(() => undefined);
      settings.close();
      await pause(50);
    }
  });
});
