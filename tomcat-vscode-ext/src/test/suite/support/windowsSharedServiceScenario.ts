import * as assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { promisify } from "node:util";
import type { TomcatExtensionApi } from "../../../extension";
import { TEST_DEFAULT_CWD_ENV, TEST_PATH_ENV } from "../../../constants";
import { SettingsFrameDriver } from "./workbenchFindDriver";

/** Runs on macOS for local preparation and on Windows for actual platform acceptance. */
export async function assertSharedServiceLifecycle(getApi: () => Promise<TomcatExtensionApi>): Promise<void> {
  const binary = process.env[TEST_PATH_ENV];
  const cwd = process.env[TEST_DEFAULT_CWD_ENV];
  const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  assert.ok(binary && cwd && artifacts);
  const api = await getApi();
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const chat = await SettingsFrameDriver.connectFromEnvironment(".tc-shell");
  try {
    await chat.waitForSnapshot(snapshot => api.__testing.getWebviewState().ready
      && Boolean(api.__testing.getWebviewState().activeSessionId)
      && snapshot.buttons.some(button => button.testId === "session-select" && !button.disabled)
      && snapshot.buttons.some(button => button.testId === "model-select" && !button.disabled),
    "real service and editable session rendered (not just host-side readiness)");
    const sessionId = api.__testing.getWebviewState().activeSessionId!;
    await chat.fill('[data-testid="composer-input"]', "Write the lifecycle fixture file and report completion.");
    await chat.click("send-button");
    await chat.waitForSnapshot(snapshot => snapshot.text.includes("Shared service task complete.")
      && !api.__testing.getWebviewState().sessionViews[sessionId]?.busy, "real task completion");
    assert.equal(await fs.readFile(path.join(cwd, "vscode-task.txt"), "utf8"), "written by the real Tomcat tool");
    const status = async () => JSON.parse((await promisify(execFile)(binary, ["serve", "status"], { cwd, env: process.env, timeout: 15_000 })).stdout);
    const before = await status();
    assert.equal(before.running, true);
    assert.equal(before.pid, api.__testing.getServeProcessId());
    assert.ok(before.instanceId);
    const capture = async (name: string, expectedText: string) => {
      const stem = path.join(artifacts, name);
      await chat.capture(`${stem}.png`);
      // Read all artifacts here; visual acceptance also opens every final PNG.
      const png = await fs.readFile(`${stem}.png`);
      assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
      assert.ok(png.readUInt32BE(16) > 0 && png.readUInt32BE(20) > 0);
      const aria = JSON.parse(await fs.readFile(`${stem}.aria.txt`, "utf8"));
      assert.ok(aria.dom.text.includes(expectedText));
      const consoleReport = JSON.parse(await fs.readFile(`${stem}.console.json`, "utf8"));
      assert.deepEqual(consoleReport.errors, []);
    };
    await chat.setViewport(1440, 900);
    await capture("shared-service-completed", "Shared service task complete.");
    await chat.fill('[data-testid="composer-input"]', "/");
    await chat.waitForSnapshot(snapshot => snapshot.text.includes("/restart"), "restart option rendered");
    await capture("restart-menu-desktop", "/restart");
    // Match the existing visual harness: narrow the actual panel, not editor + sidebar.
    await api.__testing.executeCommand("workbench.action.toggleMaximizedAuxiliaryBar");
    try {
      await chat.setViewport(390, 844);
      await chat.waitForSnapshot(snapshot => snapshot.viewport.width >= 300, "narrow capture uses the maximized panel, not the 159px sash minimum");
      await capture("restart-menu-narrow", "/restart");
    } finally {
      await chat.setViewport(1440, 900);
      await api.__testing.executeCommand("workbench.action.toggleMaximizedAuxiliaryBar");
    }
    const option = await chat.evaluate<number>(`[...document.querySelectorAll('[data-testid="slash-command-option"]')].findIndex(node => node.id === "slash-command-terminal%3Arestart")`);
    assert.ok(option >= 0, "host restart menu option exists");
    await chat.click("slash-command-option", option);
    assert.equal(await chat.evaluate<string>(`document.querySelector('[data-testid="composer-input"]').textContent`), "/restart ");
    assert.equal((await status()).instanceId, before.instanceId, "selection only completes text");
    await chat.click("send-button");
    await chat.waitForSnapshot(snapshot => api.__testing.getWebviewState().ready
      && Number.isInteger(api.__testing.getServeProcessId()) && api.__testing.getServeProcessId() !== before.pid
      && snapshot.buttons.some(button => button.testId === "model-select" && !button.disabled), "new shared service and editable session rendered");
    const service = await status();
    assert.equal(service.running, true);
    assert.notEqual(service.instanceId, before.instanceId);
    assert.notEqual(service.pid, before.pid);
    assert.equal(service.pid, api.__testing.getServeProcessId());
    const composer = await chat.evaluate<{text: string; editable: string}>(`(() => { const node = document.querySelector('[data-testid="composer-input"]'); return {text:node.textContent, editable:node.getAttribute("contenteditable")}; })()`);
    assert.deepEqual(composer, {text:"", editable:"true"}, "cleared restart draft stays cleared after reconnect");
    await capture("restart-reconnected", "Shared service task complete.");
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "window-task.json"), JSON.stringify({
      platform: process.platform, service, beforeRestart: before, sessionId,
      assertions: ["rendered composer submission", "controlled HTTP model and real file tool", "completed text", "menu selection does not restart", "real /restart sends no model prompt (outer fixture checks request count)", "new service identity and empty editable draft", "desktop/narrow PNG/ARIA/console read back"],
    }, null, 2));
  } finally { chat.close(); } // This only closes CDP; test-electron subsequently exits the real window.
}
