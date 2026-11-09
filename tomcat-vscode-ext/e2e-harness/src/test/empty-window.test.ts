import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";

const root = path.resolve(__dirname, "../../..");
const { translate } = require(path.join(root, "out/shared/i18n")) as { translate(locale: string, key: string): string };
type Api = { __testing: {
  focusWebview(): Promise<void>;
  getResolvedExecutable(): Promise<{ source: string; executable: string }>;
  getPromptHistory(): Array<{ message: string; severity: string }>;
  getObservedWebviewErrors(): unknown[];
  getWebviewState(): { activeSessionId: string | null; connectionStatus?: string };
} };
const support = require(path.join(root, "out/test/suite/support/hostE2eScenario.js")) as {
  getTomcatExtensionApi(): Promise<Api>;
};
type Driver = {
  close(): void;
  setViewport(width: number, height: number): Promise<void>;
  capture(file: string): Promise<unknown>;
  click(testId: string): Promise<void>;
  evaluate<T>(expression: string): Promise<T>;
  waitForSnapshot(predicate: (snapshot: { text: string; buttons: Array<{ testId: string | null }> }) => boolean, message: string): Promise<unknown>;
};
const capture = require(path.join(root, "out/test/suite/support/workbenchFindDriver.js")) as {
  SettingsFrameDriver: { connectFromEnvironment(selector: string): Promise<Driver> };
  captureWorkbenchArtifacts(file: string): Promise<void>;
};

suite("Installed release first launch without a workspace", () => {
  test("shows only the open-folder guidance and leaves Settings usable", async function () {
    this.timeout(90_000);
    assert.equal(process.env.TOMCAT_VSCODE_TEST_DEFAULT_CWD, undefined);
    assert.equal(vscode.workspace.workspaceFolders?.length ?? 0, 0);
    const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
    assert.ok(artifacts);
    const api = await support.getTomcatExtensionApi();
    assert.equal((await api.__testing.getResolvedExecutable()).source, "user-bin");
    await api.__testing.focusWebview();
    // DOM readiness, not Serve.ready: workspace_required intentionally is not ready.
    const chat = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-shell");
    try {
      await chat.waitForSnapshot(s => s.buttons.some(b => b.testId === "open-project-folder"), "Open Folder guidance must render");
      assert.equal(api.__testing.getWebviewState().connectionStatus, "workspace_required");
      assert.equal(api.__testing.getWebviewState().activeSessionId, null);
      const panel = await chat.evaluate<{ text: string; buttons: string[]; connectionLabel: string | null }>(`(() => {
        const panel = document.querySelector('[data-testid="loading-state"]');
        return { text: panel?.textContent ?? '', buttons: [...panel.querySelectorAll('button')].map(button => button.textContent.trim()),
          connectionLabel: document.querySelector('[data-testid="connection-chip"]')?.getAttribute('aria-label') };
      })()`);
      assert.equal(panel.connectionLabel, translate("en", "connection.ready"));
      assert.ok(panel.text.includes(translate("en", "bridge.workspaceRequired")));
      assert.ok(!panel.text.includes(translate("en", "chat.degraded")));
      assert.ok(!panel.text.includes("unknown_session"));
      assert.deepEqual(panel.buttons, [translate("en", "host.action.openProjectFolder")]);
      assert.deepEqual(api.__testing.getPromptHistory().filter(prompt => prompt.severity === "warning"), []);
      for (const [name, width, height] of [["desktop", 1440, 900], ["narrow", 390, 844]] as const) {
        await chat.setViewport(width, height);
        await chat.capture(path.join(artifacts, `empty-window-${name}.png`));
      }
      await chat.setViewport(1440, 900);
      await chat.click("settings-button");
      const settings = await capture.SettingsFrameDriver.connectFromEnvironment(".tc-settings-shell");
      try { await settings.capture(path.join(artifacts, "empty-window-settings.png")); }
      finally { settings.close(); }
      assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
      await fs.writeFile(path.join(artifacts, "empty-window.json"), JSON.stringify({ status: "passed", workspaceFolders: 0, panel, prompts: api.__testing.getPromptHistory() }, null, 2));
    } catch (error) {
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "empty-window-failure.png")).catch(() => undefined);
      throw error;
    } finally { chat.close(); }
  });
});
