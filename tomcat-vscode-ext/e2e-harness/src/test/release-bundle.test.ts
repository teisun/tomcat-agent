import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { createHash } from "node:crypto";
import * as vscode from "vscode";
import { deniedHistoryFinding } from "./release-bundle-findings";

const extensionRoot = path.resolve(__dirname, "../../..");
type Event = { type: string; isError?: boolean; toolCallId?: string; outcome?: string };
type Api = { __testing: {
  getResolvedExecutable(): { executable: string; found: boolean; source: string };
  getServeProcessId(): number | undefined;
  getPromptHistory(): Array<{ actions: string[] }>;
  getSessionState(sessionId: string): Promise<{ model: string; modelReady?: boolean }>;
  getObservedWebviewErrors(): unknown[];
  getObservedEvents(): Event[];
  getWebviewState(): { sessionViews: Record<string, { timeline: Event[] }> };
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

suite("Native installed release bundle", () => {
  test("uses its bundled CLI through real Serve, model, policy gates, stop and reload", async function () {
    this.timeout(240_000);
    const artifacts = required("TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR");
    const events: Event[] = [];
    const knownIssues: string[] = [];
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
          try { await driver.click("send-button"); } finally { driver.close(); }
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
      const installedManifest = JSON.parse(await fs.readFile(path.resolve(path.dirname(resolved.executable), "../package.json"), "utf8"));
      const finding = deniedHistoryFinding(
        api.__testing.getWebviewState().sessionViews[sessionId]?.timeline.find(event => event.toolCallId === "release-deny")?.isError,
        installedManifest.tomcat.bundledCliVersion,
      );
      await assert.rejects(fs.stat(path.join(workspace, "release-denied.txt")), { code: "ENOENT" });
      if (finding) {
        knownIssues.push(finding);
        console.warn(`KNOWN RELEASE ISSUE: ${finding}`);
      }
      const driver = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
      try { await driver.capture(path.join(artifacts, "release-agent-box.png")); } finally { driver.close(); }
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "release-workbench.png"));
      assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ onboarding: { firstPid, model: "release-onboarding", sameProcessFirstRequest: true }, architecture: process.arch, platform: process.platform, vscode: vscode.version, resolved, events, webviewErrors: api.__testing.getObservedWebviewErrors(), knownIssues, status: knownIssues.length ? "passed_with_known_issues" : "passed" }, null, 2));
    } catch (error) {
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ status: "failed", error: String(error), events: [...events, ...(testing?.getObservedEvents() ?? [])], webviewErrors: testing?.getObservedWebviewErrors() ?? [] }, null, 2));
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "release-failure.png")).catch(captureError => console.warn(captureError));
      throw error;
    }
  });
});
