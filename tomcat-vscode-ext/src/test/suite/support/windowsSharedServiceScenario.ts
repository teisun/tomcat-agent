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
    const cli = await promisify(execFile)(binary, ["serve", "status"], { cwd, env: process.env, timeout: 15_000 });
    const service = JSON.parse(cli.stdout);
    assert.equal(service.running, true);
    assert.equal(service.pid, api.__testing.getServeProcessId());
    assert.ok(service.instanceId);
    await chat.setViewport(1440, 900);
    const stem = path.join(artifacts, "shared-service-completed");
    await chat.capture(`${stem}.png`);
    // Read back all three artifacts; later human/agent visual acceptance still reads the PNG itself.
    const png = await fs.readFile(`${stem}.png`);
    assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
    assert.ok(png.readUInt32BE(16) > 0 && png.readUInt32BE(20) > 0);
    const aria = JSON.parse(await fs.readFile(`${stem}.aria.txt`, "utf8"));
    assert.ok(aria.dom.text.includes("Shared service task complete."));
    const consoleReport = JSON.parse(await fs.readFile(`${stem}.console.json`, "utf8"));
    assert.deepEqual(consoleReport.errors, []);
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "window-task.json"), JSON.stringify({
      platform: process.platform, service, sessionId,
      assertions: ["rendered composer submission", "controlled HTTP model and real file tool", "completed text", "PNG/ARIA/console read back"],
    }, null, 2));
  } finally { chat.close(); } // This only closes CDP; test-electron subsequently exits the real window.
}
