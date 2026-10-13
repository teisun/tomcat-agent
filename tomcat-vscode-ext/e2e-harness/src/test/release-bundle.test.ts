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
const capture = require(path.join(extensionRoot, "out/test/suite/support/workbenchFindDriver.js")) as {
  captureWorkbenchArtifacts(file: string): Promise<void>;
  SettingsFrameDriver: { connectFromEnvironment(selector: string): Promise<{ capture(file: string): Promise<unknown>; close(): void }> };
};
function required(name: string): string { const value = process.env[name]; assert.ok(value, `Missing ${name}`); return value; }

suite("Native installed release bundle", () => {
  test("uses its bundled CLI through real Serve, model, policy gates, stop and reload", async function () {
    this.timeout(120_000);
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
      async function prompt(text: string, expected: string) {
        api.__testing.clearObservedEvents();
        const completed = api.__testing.waitForEvent({ type: "agent_idle", sessionId, timeoutMs: 30_000 });
        completed.catch(() => undefined);
        await api.__testing.sendWebviewIntent({ type: "prompt", messageId: `release-${text}`, data: { sessionId, text } });
        assert.equal((await completed).outcome, "completed");
        await support.waitForWebviewDomSnapshot(api, dom =>
          dom.messageTexts.some(message => message.includes(expected)) ? dom : undefined,
        );
        events.push(...api.__testing.getObservedEvents());
      }
      await prompt("hello", "RELEASE_BUNDLE_READY");
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
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ architecture: process.arch, platform: process.platform, vscode: vscode.version, resolved, events, webviewErrors: api.__testing.getObservedWebviewErrors(), knownIssues, status: knownIssues.length ? "passed_with_known_issues" : "passed" }, null, 2));
    } catch (error) {
      await fs.writeFile(path.join(artifacts, "release-host.json"), JSON.stringify({ status: "failed", error: String(error), events: [...events, ...(testing?.getObservedEvents() ?? [])], webviewErrors: testing?.getObservedWebviewErrors() ?? [] }, null, 2));
      await capture.captureWorkbenchArtifacts(path.join(artifacts, "release-failure.png")).catch(captureError => console.warn(captureError));
      throw error;
    }
  });
});
