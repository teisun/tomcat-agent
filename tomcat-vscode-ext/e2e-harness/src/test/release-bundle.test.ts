import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { createHash } from "node:crypto";
import * as vscode from "vscode";
import { assertDeniedHistoryStatus } from "./release-bundle-findings";

const extensionRoot = path.resolve(__dirname, "../../..");
const { translate } = require(path.join(extensionRoot, "out/shared/i18n")) as { translate(locale: string, key: string): string };
type Event = { type: string; isError?: boolean; toolCallId?: string; outcome?: string };
type Api = { __testing: {
  getResolvedExecutable(): { executable: string; found: boolean; source: string };
  getServeProcessId(): number | undefined;
  getPromptHistory(): Array<{ actions: string[] }>;
  getSessionState(sessionId: string): Promise<{ model: string; modelReady?: boolean }>;
  getObservedWebviewErrors(): unknown[];
  getObservedEvents(): Event[];
  getWebviewState(): { activeSessionId: string | null; sessions: Array<{ sessionId: string; title: string | null; isPinned?: boolean }>; sessionViews: Record<string, { timeline: Event[] }> };
  clearObservedEvents(): void;
  focusWebview(): Promise<void>;
  waitForWebviewReady(): Promise<void>;
  createFreshWebviewSession(cwd: string): Promise<string>;
  reloadWebview(): Promise<void>;
  restartServe(): Promise<void>;
  sendWebviewIntent(intent: { type: string; messageId: string; data: Record<string, unknown> }): Promise<void>;
  sendWebviewDomAction(action: { kind: string; testId: string }): Promise<void>;
  waitForEvent(filter: { type: string; sessionId: string; timeoutMs: number; textIncludes?: string }): Promise<Event>;
  captureWebviewDom(): Promise<{ html: string; messageTexts: string[] }>;
} };
const support = require(path.join(extensionRoot, "out/test/suite/support/hostE2eScenario.js")) as {
  getTomcatExtensionApi(): Promise<Api>;
  waitForWebviewDomSnapshot<T>(api: Api, predicate: (snapshot: Awaited<ReturnType<Api["__testing"]["captureWebviewDom"]>>) => T | undefined): Promise<T>;
};
type FrameSnapshot = { text: string; viewport: { width: number; height: number }; buttons: Array<{ testId: string | null; disabled: boolean }> };
type FrameDriver = {
  capture(file: string): Promise<unknown>; close(): void;
  evaluate<T>(expression: string): Promise<T>;
  click(testId: string): Promise<void>;
  fill(selector: string, value: string): Promise<void>;
  setViewport(width: number, height: number): Promise<void>;
  waitForSnapshot(predicate: (snapshot: FrameSnapshot) => boolean, description: string): Promise<FrameSnapshot>;
};
const capture = require(path.join(extensionRoot, "out/test/suite/support/workbenchFindDriver.js")) as {
  captureWorkbenchArtifacts(file: string): Promise<void>;
  SettingsFrameDriver: { connectFromEnvironment(selector: string): Promise<FrameDriver> };
};
function required(name: string): string { const value = process.env[name]; assert.ok(value, `Missing ${name}`); return value; }

async function assertLegacyUpgradeWorkflow(api: Api, artifacts: string) {
  const sessionId = required("TOMCAT_RELEASE_UPGRADE_SESSION");
  const planId = required("TOMCAT_RELEASE_UPGRADE_PLAN");
  const root = path.join(required("HOME"), ".tomcat");
  const configBefore = await fs.readFile(path.join(root, "tomcat.config.toml"), "utf8");
  const modelsBefore = await fs.readFile(path.join(root, "models.toml"), "utf8");
  // Webview readiness precedes asynchronous Serve bootstrap/history hydration.
  await support.waitForWebviewDomSnapshot(api, dom => dom.messageTexts.some(text => text.includes("legacy fixture conversation")) ? true : undefined);
  const firstPid = api.__testing.getServeProcessId();
  assert.ok(firstPid);
  assert.ok(api.__testing.getWebviewState().sessions.some(session => session.sessionId === sessionId));
  assert.equal((await api.__testing.getSessionState(sessionId)).model, "gpt-5.4");
  assert.equal((await api.__testing.getSessionState(sessionId)).modelReady, true);
  const chat = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
  await vscode.commands.executeCommand("workbench.action.toggleMaximizedAuxiliaryBar");
  const shots: string[] = [];
  const captureBoth = async (name: string) => {
    for (const [viewport, width, height] of [["wide", 1440, 900], ["narrow", 390, 844]] as const) {
      await chat.setViewport(width, height);
      const file = path.join(artifacts, `${name}-${viewport}.png`);
      await chat.capture(file); shots.push(file);
    }
    await chat.setViewport(1440, 900);
  };
  try {
    await chat.waitForSnapshot(snapshot => snapshot.text.includes("gpt-5.4"), "Upgraded model must be visible");
    await captureBoth("upgrade-ready");
    await api.__testing.sendWebviewIntent({ type: "prompt", messageId: "upgrade-start", data: { sessionId, text: "Implement the saved upgrade plan now." } });
    await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "approval-option-plan-start-start"), "Start confirmation must render");
    assert.equal(await chat.evaluate<boolean>("!!document.querySelector('[data-testid=\"approval-option-plan-start-__custom__\"]')"), false);
    await captureBoth("start-plan-confirmation");
    await chat.click("approval-option-plan-start-start");
    await chat.click("approval-continue");
    await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "approval-option-upgrade-question-brief"), "Executing question must render");
    const planPath = path.join(root, "plans", `${planId}.plan.md`);
    assert.ok((await fs.readFile(planPath, "utf8")).includes("state: executing"));
    assert.equal(await fs.readFile(path.join(root, "temp/upgrade-execution.txt"), "utf8"), "upgraded execution");
    await captureBoth("execution-question");
    const completed = api.__testing.waitForEvent({ type: "agent_idle", sessionId, timeoutMs: 30_000 });
    void completed.catch(() => undefined);
    await chat.click("approval-option-upgrade-question-brief");
    await chat.click("approval-continue");
    assert.equal((await completed).outcome, "completed");
    await chat.waitForSnapshot(snapshot => snapshot.text.includes("UPGRADE_EXECUTION_COMPLETE"), "Answer must resume execution");
    assert.ok((await fs.readFile(planPath, "utf8")).includes("state: completed"));
    assert.equal(api.__testing.getServeProcessId(), firstPid);
    await captureBoth("execution-continued");

    const fresh = await api.__testing.createFreshWebviewSession(required("TOMCAT_VSCODE_TEST_DEFAULT_CWD"));
    await chat.click("session-select");
    const pick = async (id: string) => {
      const title = api.__testing.getWebviewState().sessions.find(session => session.sessionId === id)?.title ?? id;
      assert.equal(await chat.evaluate<boolean>(`(() => { const row = [...document.querySelectorAll('[data-testid="session-option"]')].find(row => row.getAttribute('title') === ${JSON.stringify(title)}); if (!row) return false; row.click(); return true; })()`), true);
      await chat.waitForSnapshot(() => api.__testing.getWebviewState().activeSessionId === id, "Session selection must settle");
      assert.equal(await chat.evaluate<boolean>("!!document.querySelector('[data-testid=\"session-dropdown\"]')"), true);
    };
    await pick(sessionId); await pick(fresh); await pick(sessionId);
    const rowAction = async (icon: string) => {
      const title = api.__testing.getWebviewState().sessions.find(session => session.sessionId === sessionId)?.title ?? sessionId;
      assert.equal(await chat.evaluate<boolean>(`(() => { const row = [...document.querySelectorAll('.tc-session-row')].find(row => row.querySelector('[data-testid="session-option"]')?.getAttribute('title') === ${JSON.stringify(title)}); const button = row?.querySelector('.codicon-' + ${JSON.stringify(icon)})?.closest('button'); if (!button || button.disabled) return false; button.focus(); button.click(); return true; })()`), true);
    };
    await rowAction("pin");
    await chat.waitForSnapshot(() => !!api.__testing.getWebviewState().sessions.find(session => session.sessionId === sessionId)?.isPinned, "Pin must persist");
    await rowAction("trash");
    await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "delete-session-cancel"), "Delete dialog must render");
    await chat.click("delete-session-cancel");
    assert.equal(await chat.evaluate<boolean>("!!document.querySelector('[data-testid=\"session-dropdown\"]') && document.activeElement !== document.body"), true);
    await captureBoth("session-menu");
    await chat.click("settings-button");
    assert.equal(await chat.evaluate<boolean>("!!document.querySelector('[data-testid=\"session-dropdown\"]')"), false);
    assert.equal(await fs.readFile(path.join(root, "tomcat.config.toml"), "utf8"), configBefore);
    assert.equal(await fs.readFile(path.join(root, "models.toml"), "utf8"), modelsBefore);
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    return { tag: "ext-v0.1.90", firstPid, sessionId, planId, sameProcessChat: true, screenshots: shots };
  } finally { chat.close(); }
}

suite("Native installed release bundle", () => {
  test("uses its bundled CLI through real Serve, model, policy gates, stop and reload", async function () {
    this.timeout(240_000);
    const artifacts = required("TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR");
    const events: Event[] = [];
    const deniedHistoryScreenshots: string[] = [];
    let testing: Api["__testing"] | undefined;
    try {
      console.log("Release acceptance: activating the installed extension");
      const api = await support.getTomcatExtensionApi();
      console.log("Release acceptance: activation completed; checking the bundled executable");
      testing = api.__testing;
      const workspace = required("TOMCAT_VSCODE_TEST_DEFAULT_CWD");
      const resolved = api.__testing.getResolvedExecutable();
      assert.equal(resolved.source, "bundled");
      assert.equal(resolved.found, true);
      assert.equal(await fs.realpath(resolved.executable), await fs.realpath(required("TOMCAT_EXPECT_RELEASE_BINARY")));
      assert.equal(createHash("sha256").update(await fs.readFile(resolved.executable)).digest("hex"), required("TOMCAT_EXPECT_RELEASE_CLI_SHA256"));
      assert.equal(process.arch, required("TOMCAT_EXPECT_RELEASE_HOST_ARCH"), "Electron and verifier must share the native architecture");
      await api.__testing.focusWebview();
      await api.__testing.waitForWebviewReady();
      if (process.env.TOMCAT_RELEASE_UPGRADE_SESSION) {
        const upgrade = await assertLegacyUpgradeWorkflow(api, artifacts);
        await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ status: "passed", upgrade, resolved, architecture: process.arch, platform: process.platform, vscode: vscode.version, webviewErrors: api.__testing.getObservedWebviewErrors() }, null, 2));
        return;
      }
      const sessionId = await api.__testing.createFreshWebviewSession(workspace);
      const configPath = path.join(required("HOME"), ".tomcat/tomcat.config.toml");
      for (const name of ["tomcat.config.toml", "models.toml", ".env"]) {
        await assert.rejects(fs.stat(path.join(path.dirname(configPath), name)), { code: "ENOENT" });
      }
      const firstPid = api.__testing.getServeProcessId();
      assert.ok(firstPid, "real Serve must be running before model configuration");
      assert.equal((await api.__testing.getSessionState(sessionId)).modelReady, false);
      assert.ok(!api.__testing.getPromptHistory().some(prompt => prompt.actions.includes("Start Setup")));
      assert.equal(vscode.window.terminals.length, 0, "cold start must not launch init in a terminal");
      const requestCount = async () => {
        const response = await fetch(`${required("TOMCAT_ONBOARDING_BASE_URL")}/_test/request-count`);
        assert.equal(response.status, 200);
        return (await response.json() as { count: number }).count;
      };
      assert.equal(await requestCount(), 0);
      const chat = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
      try {
        await chat.setViewport(1440, 900);
        await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "configure-model"), "Inline configuration card must appear");
        assert.equal(await chat.evaluate<boolean>(`(() => {
          const card = document.querySelector('[data-testid="model-setup-card"]');
          return !!card && card.matches(".tc-empty-state") && !card.closest('[data-testid="composer-footer"]');
        })()`), true, "An empty session must show the card in the empty-state slot, not above the composer");
        await chat.fill('[data-testid="composer-input"]', "hello");
        assert.equal(await chat.evaluate<boolean>('document.querySelector(\'[data-testid="send-button"]\').disabled'), true);
        await chat.capture(path.join(artifacts, "onboarding-blocked-desktop.png"));
        await chat.setViewport(760, 900);
        await chat.capture(path.join(artifacts, "onboarding-blocked-narrow.png"));
        await chat.click("configure-model");
        const settings = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-settings-shell");
        try {
          await settings.setViewport(1440, 900);
          await settings.click("settings-add-model");
          await settings.click("settings-mode-relay");
          await settings.fill('[data-testid="settings-model-base-url"]', required("TOMCAT_ONBOARDING_BASE_URL").replace("127.0.0.1", "localhost"));
          await settings.fill('[data-testid="settings-model-name"]', "deepseek-v4-flash");
          await settings.fill('[data-testid="settings-model-alias"]', "release-onboarding");
          await settings.fill('[data-testid="settings-api-key-input"]', "local-onboarding-dummy-key");
          assert.equal(await settings.evaluate<boolean>('document.querySelector(\'[data-testid="settings-set-default-model"]\').checked'), true);
          await settings.capture(path.join(artifacts, "onboarding-model-form-desktop.png"));
          await settings.evaluate('document.querySelector(\'[data-testid="settings-set-default-model"]\').scrollIntoView({ block: "center" })');
          await settings.capture(path.join(artifacts, "onboarding-default-desktop.png"));
          await settings.setViewport(800, 900);
          await settings.capture(path.join(artifacts, "onboarding-model-form-narrow.png"));
          await settings.evaluate('document.querySelector(\'[data-testid="settings-set-default-model"]\').scrollIntoView({ block: "center" })');
          await settings.capture(path.join(artifacts, "onboarding-default-narrow.png"));
          await settings.click("settings-save-model");
          await settings.waitForSnapshot(snapshot => snapshot.text.includes("release-onboarding") && !snapshot.buttons.some(button => button.testId === "settings-save-model"), "Save must finish before the form closes");
          await settings.setViewport(1440, 900);
          assert.equal(await settings.evaluate<boolean>('!!document.querySelector(\'[data-testid="settings-default-release-onboarding"]\')'), true, "The saved model must carry the Default chip");
          await settings.capture(path.join(artifacts, "onboarding-model-saved.png"));
          await settings.click("settings-edit-release-onboarding");
          await settings.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "settings-save-model"), "Edit form must open");
          assert.equal(await settings.evaluate<boolean>('!document.querySelector(\'[data-testid="settings-set-default-model"]\')'), true, "Edit must not offer Set as default model");
          await settings.click("settings-close-model-form");
        } finally { settings.close(); }
        await vscode.commands.executeCommand("workbench.action.closeActiveEditor");
        await api.__testing.focusWebview();
        // Settings used another CDP session; restore this session's desktop metrics.
        await chat.setViewport(1440, 900);
        await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "send-button" && !button.disabled) && !snapshot.buttons.some(button => button.testId === "configure-model"), "Configuration must unlock the existing draft without restart");
        assert.equal(await chat.evaluate<string>('document.querySelector(\'[data-testid="composer-input"]\').textContent'), "hello");
        assert.equal(api.__testing.getServeProcessId(), firstPid);
        assert.equal(await requestCount(), 0, "Saving configuration must not automatically submit the draft");
        await chat.capture(path.join(artifacts, "onboarding-ready.png"));
      } finally { chat.close(); }
      assert.equal((await api.__testing.getSessionState(sessionId)).model, "release-onboarding");
      async function prompt(text: string, expected: string, fromComposer = false) {
        api.__testing.clearObservedEvents();
        const completed = api.__testing.waitForEvent({ type: "agent_idle", sessionId, timeoutMs: 30_000 });
        completed.catch(() => undefined);
        if (fromComposer) {
          const driver = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
          try { await driver.setViewport(1440, 900); await driver.click("send-button"); } finally { driver.close(); }
        } else {
          await api.__testing.sendWebviewIntent({ type: "prompt", messageId: `release-${text}`, data: { sessionId, text } });
        }
        assert.equal((await completed).outcome, "completed");
        await support.waitForWebviewDomSnapshot(api, dom =>
          dom.messageTexts.some(message => message.includes(expected)) ? dom : undefined,
        );
        events.push(...api.__testing.getObservedEvents());
      }
      await prompt("hello", "RELEASE_BUNDLE_READY", true);
      assert.equal(api.__testing.getServeProcessId(), firstPid, "First model request must use the original cold-start process");
      // The existing policy test requires startup-only permission configuration.
      // Add it only AFTER proving same-PID onboarding; reuse the same HOME and model.
      await fs.appendFile(configPath, `\n[workspace]\nworkspace_roots = [${JSON.stringify(workspace)}]\n[primitive]\nauto_confirm = false\npath_rules = [{ path = ${JSON.stringify(path.join(workspace, "release-denied.txt"))}, mode = "deny" }]\n`);
      await api.__testing.restartServe();
      await api.__testing.waitForWebviewReady();
      await prompt("permitted write", "RELEASE_ALLOWED");
      assert.equal(await fs.readFile(path.join(workspace, "release-allowed.txt"), "utf8"), "allowed by workspace policy");
      assert.ok(events.some(event => event.type === "tool_execution_end" && event.toolCallId === "release-allow" && event.isError === false));
      await prompt("denied write", "RELEASE_DENIED");
      await assert.rejects(fs.stat(path.join(workspace, "release-denied.txt")), { code: "ENOENT" });
      assert.ok(events.some(event => event.type === "tool_execution_end" && event.toolCallId === "release-deny" && event.isError === true));
      await vscode.commands.executeCommand("workbench.action.toggleMaximizedAuxiliaryBar");
      const captureDeniedHistory = async (phase: string) => {
        const driver = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
        try {
          await driver.waitForSnapshot(snapshot => snapshot.text.includes(translate("en", "tool.failedWrite")), "Denied write must render as failed");
          for (const [size, width, height] of [["wide", 1440, 900], ["narrow", 390, 844]] as const) {
            await driver.setViewport(width, height);
            const rows = await driver.evaluate<Array<{ text: string; error: boolean }>>(`(() => {
              const rows = [...document.querySelectorAll('[data-testid="tool-row"]')];
              rows.find(row => row.textContent.includes('release-denied.txt'))?.scrollIntoView({ block: 'center' });
              return rows.map(row => ({ text: row.querySelector('[data-testid="tool-row-label"]')?.textContent ?? '', error: row.classList.contains('tc-tool-row--error') }));
            })()`);
            assert.ok(rows.some(row => row.error && row.text.includes(translate("en", "tool.failedWrite")) && row.text.includes("release-denied.txt")));
            assert.ok(rows.some(row => !row.error && row.text.includes(translate("en", "tool.created")) && row.text.includes("release-allowed.txt")));
            const file = path.join(artifacts, `denied-${phase}-${size}.png`);
            await driver.capture(file); deniedHistoryScreenshots.push(file);
          }
          await driver.setViewport(1440, 900);
        } finally { driver.close(); }
      };
      await captureDeniedHistory("before-reload");
      // This checks actual configured policy, not an auto-confirm shortcut or a
      // fake tool. Native interactive modal approval remains a manual acceptance.
      api.__testing.clearObservedEvents();
      const started = api.__testing.waitForEvent({ type: "message_update", sessionId, timeoutMs: 20_000, textIncludes: "RELEASE_STOP_STARTED" });
      const stopped = api.__testing.waitForEvent({ type: "agent_idle", sessionId, timeoutMs: 20_000 });
      started.catch(() => undefined); stopped.catch(() => undefined);
      await api.__testing.sendWebviewIntent({ type: "prompt", messageId: "release-stop", data: { sessionId, text: "stop this request" } });
      await started;
      await api.__testing.sendWebviewDomAction({ kind: "clickTestId", testId: "stop-button" });
      assert.equal((await stopped).outcome, "interrupted");
      events.push(...api.__testing.getObservedEvents());
      await api.__testing.reloadWebview();
      await api.__testing.waitForWebviewReady();
      await api.__testing.restartServe();
      await api.__testing.waitForWebviewReady();
      assert.equal((await api.__testing.getSessionState(sessionId)).model, "release-onboarding", "Saved selection must survive restart");
      await prompt("after reload", "RELEASE_RELOADED");
      assertDeniedHistoryStatus(
        api.__testing.getWebviewState().sessionViews[sessionId]?.timeline.find(event => event.toolCallId === "release-deny")?.isError,
      );
      assert.equal(api.__testing.getWebviewState().sessionViews[sessionId]?.timeline.find(event => event.toolCallId === "release-allow")?.isError, false);
      await assert.rejects(fs.stat(path.join(workspace, "release-denied.txt")), { code: "ENOENT" });
      await captureDeniedHistory("after-reload");
      const driver = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
      try { await driver.capture(path.join(artifacts, "release-agent-box.png")); } finally { driver.close(); }
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "release-workbench.png"));
      assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ onboarding: { firstPid, model: "release-onboarding", sameProcessFirstRequest: true }, architecture: process.arch, platform: process.platform, vscode: vscode.version, resolved, events, webviewErrors: api.__testing.getObservedWebviewErrors(), knownIssues: [], deniedHistoryScreenshots, status: "passed" }, null, 2));
    } catch (error) {
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ status: "failed", error: String(error), events: [...events, ...(testing?.getObservedEvents() ?? [])], webviewErrors: testing?.getObservedWebviewErrors() ?? [] }, null, 2));
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "release-failure.png")).catch(captureError => console.warn(captureError));
      throw error;
    }
  });
});
