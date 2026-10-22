import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { initializeServe } from "../src/serveClient/initialize";
import { setupRealServeFixture, spawnScriptedOpenAiStreamServer, sseToolCall, sseDelta, sseFinish, sseDone } from "./realServeFixture";

/** The outer launcher, not the extension host, owns model/service cleanup. */
export async function setupSharedServiceLifecycle(binary: string, systemProxy = false) {
  if (systemProxy && (process.platform !== "win32" || process.env.GITHUB_ACTIONS !== "true")) {
    throw new Error("System-proxy GUI fixture requires the explicit GitHub Windows wrapper");
  }
  const model = await spawnScriptedOpenAiStreamServer([
    { parts: [sseToolCall("lifecycle-write", "write", JSON.stringify({ path: "vscode-task.txt", content: "written by the real Tomcat tool" })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("Shared service task complete."), sseFinish("stop"), sseDone()] },
  ]);
  let real: Awaited<ReturnType<typeof setupRealServeFixture>>;
  const baseUrl = systemProxy ? model.baseUrl.replace("127.0.0.1", "tomcat-system-model.invalid") : model.baseUrl;
  try { real = await setupRealServeFixture(baseUrl, "openai", { binary }); }
  catch (error) { await model.close(); throw error; }
  const status = async () => JSON.parse((await promisify(execFile)(binary, ["serve", "status"], {
    cwd: real.workspacePath, env: real.env, timeout: 15_000,
  })).stdout);
  return {
    workspaceDir: real.workspacePath,
    env: { ...real.env, TOMCAT_E2E_SHARED_LIFECYCLE: "1", TOMCAT_VSCODE_TEST_PATH: binary,
      TOMCAT_VSCODE_TEST_DEFAULT_CWD: real.workspacePath, TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1" },
    async verifyAfterWindowExit(artifacts: string) {
      // Called only after test-electron's real GUI process exit/close has resolved.
      const before = JSON.parse(await fs.readFile(path.join(artifacts, "window-task.json"), "utf8"));
      const requests = model.capturedNonTitleRequests();
      await fs.writeFile(path.join(artifacts, "model-requests.json"), JSON.stringify(requests, null, 2));
      assert.equal(requests.length, 2);
      assert.ok(requests[0].includes("Write the lifecycle fixture file"));
      const after = await status();
      assert.equal(after.running, true, "closing VS Code must not stop the service");
      assert.equal(after.instanceId, before.service.instanceId);
      assert.equal(after.pid, before.service.pid);
      const messenger = new TomcatMessenger({ executable: binary, cwd: real.workspacePath, env: real.env });
      try {
        // Critical: a normal first request allows startup and could conceal a dead service.
        await messenger.start({ allowStart: false });
        await initializeServe(messenger);
        const opened = await messenger.request({ type: "switch_session", sessionId: before.sessionId });
        assert.equal(opened.success, true);
        const state = await messenger.request({ type: "get_state", sessionId: before.sessionId });
        assert.equal(state.success, true);
        const final = await status();
        assert.equal(final.instanceId, before.service.instanceId);
        assert.equal(final.pid, before.service.pid);
        await fs.writeFile(path.join(artifacts, "window-exit-lifecycle.json"), JSON.stringify({
          status: "passed", platform: process.platform, before, after, final,
          proof: "real test-electron GUI process exited; status and Native IPC without allowing startup; same service identity",
        }, null, 2));
      } finally { await messenger.disposeAsync(); }
    },
    async cleanup() { try { await real.cleanup(); } finally { await model.close(); } },
  };
}
