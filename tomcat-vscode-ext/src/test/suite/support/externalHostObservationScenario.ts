import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import type { TomcatExtensionApi } from "../../../extension";
import { readExternalObservationFixture } from "./externalObservationFixture";
import { SettingsFrameDriver } from "./workbenchFindDriver";

async function until(predicate: () => boolean | Promise<boolean>, description: string): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error(`External observation timed out: ${description}`);
}

/** Runs against a task started by the real host, never creates an ACP test client. */
export async function assertExternalHostObservation(getApi: () => Promise<TomcatExtensionApi>): Promise<void> {
  const file = process.env.TOMCAT_E2E_EXTERNAL_OBSERVATION;
  const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  assert.ok(file && artifacts);
  const fixture = await readExternalObservationFixture(file);
  await fs.mkdir(artifacts, { recursive: true });
  const api = await getApi();
  await api.__testing.focusWebview();
  await api.__testing.waitForWebviewReady();
  await until(() => api.__testing.getWebviewState().ready && !!api.__testing.getWebviewState().activeSessionId, "window ready");
  const chat = await SettingsFrameDriver.connectFromEnvironment(".tc-shell");
  try {
    await chat.setViewport(1440, 900);
    await chat.click("session-select");
    if (!fixture.visible) {
      assert.equal(api.__testing.getWebviewState().sessions.some(row => row.sessionId === fixture.sessionId), false);
      await assert.rejects(api.__testing.getSessionState(fixture.sessionId), /unknown_session/);
      const before = api.__testing.getWebviewState().activeSessionId;
      await assert.rejects(api.__testing.sendWebviewIntent({ messageId: "external-foreign-open", type: "switchSession", data: { sessionId: fixture.sessionId } }), /unknown_session/);
      assert.equal(api.__testing.getWebviewState().activeSessionId, before);
      assert.equal(api.__testing.getObservedEvents().some(event => event.sessionId === fixture.sessionId), false);
      await chat.capture(path.join(artifacts, "foreign-project.png"));
    } else {
      await until(() => api.__testing.getWebviewState().sessions.some(row => row.sessionId === fixture.sessionId), "host in ordinary disk list");
      await chat.click("session-select"); // Close the inspected dropdown before opening the conversation.
      // Use the same webview intent as the existing row; no observer-only API.
      await api.__testing.sendWebviewIntent({ messageId: "external-host-open", type: "switchSession", data: { sessionId: fixture.sessionId } });
      await until(() => api.__testing.getWebviewState().activeSessionId === fixture.sessionId, "host selected");
      const state = () => api.__testing.getWebviewState().sessionViews[fixture.sessionId];
      await until(() => state()?.timeline.some(item => item.type === "message" && item.text.includes(fixture.prefix)) === true, "prefix from before this window opened");
      assert.equal(state().access, "observe");
      assert.equal(state().busy, true, "the real host task is still running");
      await until(() => chat.evaluate<boolean>(`document.body.innerText.includes(${JSON.stringify(fixture.prefix)}) && !!document.querySelector('[data-testid="observer-notice"]')`), "rendered prefix and notice");
      const checkReadOnly = async () => {
        const dom = await chat.evaluate<{ editable: string; disabled: string[]; fits: boolean }>(`({
          editable: document.querySelector('[data-testid="composer-input"]')?.getAttribute('contenteditable'),
          disabled: [...document.querySelectorAll('button[disabled]')].map(button => button.dataset.testid),
          fits: [...document.querySelectorAll('[data-testid="mode-select"], [data-testid="model-select"], [data-testid="stop-button"]')].every(button => { const r = button.getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth; })
        })`);
        assert.equal(dom.editable, "false");
        for (const id of ["mode-select", "model-select", "thinking-level-select", "stop-button"]) assert.ok(dom.disabled.includes(id), `${id} disabled`);
        assert.ok(dom.fits);
      };
      await checkReadOnly();
      await chat.capture(path.join(artifacts, "observer-desktop.png"));
      await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
      await chat.setViewport(390, 844);
      await checkReadOnly();
      await chat.capture(path.join(artifacts, "observer-narrow.png"));
      assert.equal(state().timeline.filter(item => item.type === "message" && item.text.includes(fixture.prefix)).length, 1);
    }
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "external-host-journey.json"), JSON.stringify({ sessionId: fixture.sessionId, workspacePath: fixture.workspacePath, visible: fixture.visible, access: api.__testing.getWebviewState().sessionViews[fixture.sessionId]?.access, webviewErrors: api.__testing.getObservedWebviewErrors() }, null, 2));
  } finally {
    chat.close(); // Closing the observer must not cancel the external host's task.
  }
}
