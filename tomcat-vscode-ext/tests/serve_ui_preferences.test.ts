import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { readFile, rename } from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { createRealServeMessenger, ensureTomcatBinary, spawnScriptedOpenAiStreamServer, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const exec = promisify(execFile);

describe("real Serve language preferences", () => {
  it("persists the shared preference, changes new output without restarting, and is read by a new CLI", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    runtime.messenger.updateOptions({ env: { ...runtime.fixture.env, TOMCAT_HOST_LOCALE: "zh-CN", TOMCAT__UI__LANGUAGE: undefined } });
    try {
      const initial = await initializeServe(runtime.messenger);
      expect(initial.uiPreferences).toEqual({ language: "auto", effective: "zh-CN", envOverride: false });
      const pid = runtime.messenger.pid;
      const response = await runtime.messenger.request({ type: "set_ui_language", language: "en" });
      expect(response.success).toBe(true);
      expect(response.payload).toEqual({ language: "en", effective: "en", envOverride: false });
      expect(runtime.messenger.pid).toBe(pid);
      const config = path.join(runtime.fixture.homePath, ".tomcat", "tomcat.config.toml");
      expect(await readFile(config, "utf8")).toMatch(/\[ui\]\s+language = "en"/u);
      const cli = await exec(await ensureTomcatBinary(), ["--help"], { cwd: runtime.fixture.workspacePath, env: runtime.fixture.env });
      expect(cli.stdout).toContain("Usage:");
      expect(cli.stdout).toContain("global conversation");
      const before = await runtime.messenger.request({ type: "list_sessions", scope: "disk" });
      await rename(config, `${config}.held`);
      const failed = await runtime.messenger.request({ type: "set_ui_language", language: "zh-CN" });
      expect(failed.success).toBe(false);
      expect(failed.error).toContain("Configuration file not found");
      await rename(`${config}.held`, config);
      const after = await runtime.messenger.request({ type: "list_sessions", scope: "disk" });
      expect(after.payload).toEqual(before.payload);
    } finally { await runtime.cleanup(); await server.close(); }
  }, 30_000);
});
