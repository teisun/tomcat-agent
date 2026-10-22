// Runner-neutral fixture setup; shared model transport also runs inside VS Code tests.
import * as os from "node:os";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
export { spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, sseToolCall } from "../src/test/suite/support/scriptedModel";
export type { ScriptedPart, ScriptedResponse } from "../src/test/suite/support/scriptedModel";

const execFileAsync = promisify(execFile);
export type LlmApi = "openai" | "openai-responses";

export function serveFixtureEnvironment(homePath: string, inherited: NodeJS.ProcessEnv = process.env, coldStart = false): NodeJS.ProcessEnv {
  const env = { ...inherited };
  // Only the child with a fixture-owned disposable HOME may leave nested-agent mode.
  env.TOMCAT_AGENT_ACTIVE = undefined;
  for (const key of Object.keys(env)) {
    if (key.startsWith("TOMCAT__") || /_API_KEY$/i.test(key)) env[key] = undefined;
  }
  return {
    ...env, HOME: homePath, USERPROFILE: homePath, TOMCAT_HOST_LOCALE: "en", SHELL: "/bin/zsh",
    TOMCAT__STORAGE__WORK_DIR: path.join(homePath, ".tomcat"),
    TOMCAT__CONTEXT__COMPACTION_MODEL: coldStart ? undefined : "gpt-5.4",
    TOMCAT__LLM__DEFAULT_MODEL: coldStart ? undefined : "gpt-5.4",
    OPENAI_API_KEY: coldStart ? undefined : "dummy-key",
    ALL_PROXY: "", HTTPS_PROXY: "", HTTP_PROXY: "", all_proxy: "", https_proxy: "", http_proxy: "",
    NO_PROXY: "127.0.0.1,localhost", no_proxy: "127.0.0.1,localhost",
  };
}

export type FixtureSetup = {
  binary: string;
  coldStart?: boolean;
  /** Explicit test configuration only; this fixture never reads shell proxy variables. */
  llmProxy?: string;
  runCli?: (binary: string, args: string[], options: {
    cwd: string; env: NodeJS.ProcessEnv; timeout: number; killSignal?: NodeJS.Signals;
  }) => Promise<unknown>;
};
export async function setupRealServeFixture(baseUrl: string, api: LlmApi, setup: FixtureSetup) {
  const proxy = setup.llmProxy?.trim();
  if (proxy !== undefined) {
    try {
      const url = new URL(proxy);
      if (!["http:", "https:"].includes(url.protocol) || !url.hostname) throw new Error();
    } catch {
      // Proxy URLs may contain credentials: do not echo the input or URL parser error.
      throw new Error("Test llm.proxy must be an absolute HTTP(S) proxy URL");
    }
    if (setup.coldStart) throw new Error("A cold-start fixture cannot save llm.proxy before initialization");
  }
  const binary = setup.binary;
  const runCli = setup.runCli ?? execFileAsync;
  // macOS's default per-user temp directory exceeds sockaddr_un's path limit.
  const homePath = await mkdtemp(path.join(process.platform === "darwin" ? "/tmp" : os.tmpdir(), "tomcat-vscode-ext-"));
  const workspacePath = path.join(homePath, "workspace");
  const env = serveFixtureEnvironment(homePath, process.env, setup.coldStart);
  // Retry transient ENOTEMPTY/EBUSY during disposable fixture cleanup; permanent errors still reject.
  let cleanupTask: Promise<void> | undefined;
  const cleanup = () => cleanupTask ??= (async () => {
    // Closing a messenger no longer owns the service process. Only this isolated
    // fixture may stop its service, and it must do so before removing its HOME.
    await runCli(binary, ["serve", "stop"], { cwd: workspacePath, env, timeout: 35_000 });
    await rm(homePath, { force: true, recursive: true, maxRetries: 3, retryDelay: 100 });
  })().catch(error => { cleanupTask = undefined; throw error; });
  try {
    await mkdir(workspacePath, { recursive: true });
    if (setup.coldStart) return { cleanup, env, homePath, workspacePath };
    await runCli(binary, ["init"], { cwd: workspacePath, env, timeout: 15_000, killSignal: "SIGKILL" });
    if (proxy !== undefined) {
      // init generates this fixture's [llm] table without a proxy entry. CLI config
      // set cannot create a missing optional key, so seed it here before startup.
      const configPath = path.join(homePath, ".tomcat", "tomcat.config.toml");
      const config = await readFile(configPath, "utf8");
      if (!/^\[llm\]\r?$/m.test(config)) throw new Error("Fixture config is missing its llm table");
      await writeFile(configPath, config.replace(/^\[llm\]\r?$/m, () => `[llm]\nproxy = ${JSON.stringify(proxy)}`), "utf8");
    }
    // Shared serve deliberately does not inherit an arbitrary starter's model key.
    await runCli(binary, ["model", "key", "set", "openai", "dummy-key"], { cwd: workspacePath, env, timeout: 15_000 });
    await writeFile(path.join(homePath, ".tomcat", "mcp.json"), '{"mcpServers":{}}\n', "utf8");
    await writeFile(path.join(homePath, ".tomcat", "models.toml"), `[[models]]
id = "gpt-5.4"
api = "${api}"
provider = "openai"
base_url = "${baseUrl}"
capabilities = { vision = false, files = false, tools = true, reasoning = true, web_search = false }
`, "utf8");
    await runCli(binary, ["model", "default", "gpt-5.4"], { cwd: workspacePath, env, timeout: 15_000 });
    return { cleanup, env, homePath, workspacePath };
  } catch (error) {
    try { await cleanup(); } catch (cleanupError) { console.warn("serve fixture cleanup failed after setup error", cleanupError); }
    throw error;
  }
}
