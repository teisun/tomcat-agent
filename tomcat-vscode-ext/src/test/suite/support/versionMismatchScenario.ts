import * as assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { promisify } from "node:util";
import type { TomcatExtensionApi } from "../../../extension";
import { TEST_DEFAULT_CWD_ENV, TEST_PATH_ENV } from "../../../constants";
import { en } from "../../../shared/i18n/en";
import { SettingsFrameDriver, type SettingsDomEvidence } from "./workbenchFindDriver";

/** Real VS Code recovery, with either two real CLI builds or explicit metadata fault injection. */
export async function assertVersionMismatchRecovery(getApi: () => Promise<TomcatExtensionApi>): Promise<void> {
  const binary = process.env[TEST_PATH_ENV];
  const cwd = process.env[TEST_DEFAULT_CWD_ENV];
  const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  assert.ok(binary && cwd && artifacts);
  await fs.mkdir(artifacts, { recursive: true });
  const cli = (args: string[]) => promisify(execFile)(binary, args, { cwd, env: process.env, timeout: 60_000 });
  const before = JSON.parse((await cli(["serve", "start"])).stdout);
  const version = (await cli(["--version"])).stdout.trim().match(/^tomcat\s+(\S+)$/u)?.[1];
  assert.ok(version);
  assert.notEqual(before.serverVersion, version);
  const serviceBinary = process.env.TOMCAT_E2E_SERVICE_BINARY;
  let serviceVersion: string | undefined;
  if (serviceBinary) {
    serviceVersion = (await promisify(execFile)(serviceBinary, ["--version"], { cwd, env: process.env, timeout: 60_000 }))
      .stdout.trim().match(/^tomcat\s+(\S+)$/u)?.[1];
    assert.ok(serviceVersion);
    assert.equal(before.serverVersion, serviceVersion, "running version must come from the actual other executable");
  }
  const api = await getApi();
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  const chat = await SettingsFrameDriver.connectFromEnvironment(".tc-shell");
  try {
    const failed = await chat.waitForSnapshot(snapshot => snapshot.buttons.some(button => button.testId === "restart-shared-service"), "version failure and its explicit restart action");
    assert.ok(failed.text.includes(before.serverVersion) && failed.text.includes(version));
    assert.equal(failed.buttons.some(button => button.testId === "retry-connection"), false);
    assert.equal(failed.buttons.find(button => button.testId === "restart-shared-service")?.name, en["host.action.restartSharedService"]);
    assert.equal(api.__testing.getWebviewState().ready, false);

    const assertActionFits = (dom: SettingsDomEvidence) => {
      const action = dom.buttons.find(button => button.testId === "restart-shared-service");
      assert.ok(action && !action.disabled);
      assert.ok(action.x >= 0 && action.x + action.width <= dom.viewport.width + 1, `Restart action is clipped: ${JSON.stringify(dom)}`);
    };
    await chat.setViewport(1440, 900);
    assertActionFits(await chat.capture(path.join(artifacts, "version-mismatch-desktop.png")));
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(390, 844);
    assertActionFits(await chat.capture(path.join(artifacts, "version-mismatch-narrow.png")));

    // Click the rendered action once. No test-side reconnect or confirmation bypass.
    await chat.click("restart-shared-service");
    await chat.waitForSnapshot(snapshot => api.__testing.getWebviewState().ready
      && snapshot.buttons.some(button => button.testId === "send-button")
      && !snapshot.buttons.some(button => button.testId === "restart-shared-service"), "reconnected editable composer");
    const after = JSON.parse((await cli(["serve", "status"])).stdout);
    assert.equal(after.serverVersion, version);
    assert.notEqual(after.instanceId, before.instanceId, "the actual service process must have restarted");
    const sessionId = api.__testing.getWebviewState().activeSessionId;
    assert.ok(sessionId);
    await api.__testing.getSessionState(sessionId); // A real request on the recovered native connection.
    assert.equal(await chat.evaluate("document.querySelector('[data-testid=\"composer-input\"]')?.getAttribute('contenteditable')"), "true");
    await chat.capture(path.join(artifacts, "version-recovered-narrow.png"));
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "version-recovery.json"), JSON.stringify({
      fixture: serviceBinary ? "two independently compiled CLIs; real VS Code, Rust restart and recovered IPC"
        : "fault-injected version metadata; real VS Code, Rust restart and recovered IPC",
      binaries: serviceBinary ? { client: { path: binary, version }, service: { path: serviceBinary, version: serviceVersion } } : undefined,
      before, after, sessionId,
      assertions: ["version details", "restart replaces retry", "desktop and narrow action fits", "one rendered click without a second confirmation", "new service instance", "native request after recovery"],
    }, null, 2));
  } finally { chat.close(); }
}
