import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { runTests } from "@vscode/test-electron";
import { resolveVsCodeCli, resolveVsCodeExecutable, seedChatUserSettings } from "./e2eHostFixture";
import { packageVsix } from "./package-vsix";
import { runVsCodeGuiTests } from "./vscodeLaunchEnv";

const outsideAgentEnv: NodeJS.ProcessEnv = { ...process.env };
for (const key of Object.keys(outsideAgentEnv)) {
  // Only standalone instances with an owned disposable HOME receive this env.
  if (key === "TOMCAT_AGENT_ACTIVE" || key.startsWith("TOMCAT__") || /_API_KEY$/i.test(key)) outsideAgentEnv[key] = undefined;
}
Object.assign(outsideAgentEnv, {
  ALL_PROXY: "", HTTPS_PROXY: "", HTTP_PROXY: "", NO_PROXY: "127.0.0.1,localhost",
  all_proxy: "", https_proxy: "", http_proxy: "", no_proxy: "127.0.0.1,localhost",
});
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function sha256(file: string): Promise<string> { return createHash("sha256").update(await fs.readFile(file)).digest("hex"); }

async function seedRealServeSettings(userDataDir: string, executable: string, workspaceDir: string): Promise<void> {
  await seedChatUserSettings(userDataDir);
  const file = path.join(userDataDir, "User", "settings.json");
  const current = JSON.parse(await fs.readFile(file, "utf8")) as Record<string, unknown>;
  await fs.writeFile(file, `${JSON.stringify({
    ...current, "extensions.autoCheckUpdates": false, "extensions.autoUpdate": "off",
    "security.workspace.trust.enabled": false, "telemetry.telemetryLevel": "off",
    "tomcat.path": executable, "tomcat.session.defaultCwd": workspaceDir,
    "update.mode": "none", "workbench.startupEditor": "none", "workbench.tips.enabled": false,
  }, null, 2)}\n`);
}

async function startHttpFixture(binary: string, temporaryRoot: string, artifactsRoot: string) {
  const addressFile = path.join(temporaryRoot, "mcp-bound-address");
  const child = spawn(binary, ["--port", "0", "--faults"], {
    env: { ...outsideAgentEnv, MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE: addressFile },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  let spawnError: Error | undefined;
  child.on("error", (error) => { spawnError = error; });
  child.stderr.on("data", (chunk) => { stderr = (stderr + String(chunk)).slice(-32_768); });
  const stop = async () => {
    if (child.pid && child.exitCode === null && child.signalCode === null) {
      await new Promise<void>((resolve) => {
        const force = setTimeout(() => child.kill("SIGKILL"), 5_000);
        child.once("exit", () => { clearTimeout(force); resolve(); });
        child.kill("SIGTERM");
      });
    }
    await fs.writeFile(path.join(artifactsRoot, "mcp-fixture-process.json"), JSON.stringify({ pid: child.pid, exitCode: child.exitCode, signal: child.signalCode, stderr }, null, 2));
  };
  try {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null || child.signalCode !== null) throw new Error(`MCP fixture exited before ready: ${stderr}`);
      const address = await fs.readFile(addressFile, "utf8").catch((error: NodeJS.ErrnoException) => {
        if (error.code === "ENOENT") return ""; throw error;
      });
      // Creation/truncation can precede the write; never accept an empty address.
      if (/^127\.0\.0\.1:[1-9][0-9]*$/.test(address.trim())) return { url: `http://${address.trim()}`, stop };
      await pause(25);
    }
    throw new Error("Timed out waiting for the controlled MCP fixture");
  } catch (error) { await stop(); throw error; }
}

async function assetHashes(root: string, prefix = ""): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const entry of await fs.readdir(path.join(root, prefix), { withFileTypes: true })) {
    const relative = path.join(prefix, entry.name);
    if (entry.isDirectory()) Object.assign(result, await assetHashes(root, relative));
    else if (entry.isFile() && /\.(js|css)$/.test(entry.name)) result[relative] = await sha256(path.join(root, relative));
  }
  return result;
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.join(extensionRoot, "e2e-harness");
  const executable = path.resolve(process.env.TOMCAT_CONNECTORS_ACCEPT_BINARY ?? path.join(extensionRoot, "../tomcat/target/debug/tomcat"));
  const mcpBinary = path.resolve(process.env.TOMCAT_CONNECTORS_ACCEPT_MCP_BINARY ?? path.join(extensionRoot, "../tomcat/target/debug/test_streamable_http_server"));
  await Promise.all([fs.access(executable), fs.access(mcpBinary)]);
  // Keep macOS's AF_UNIX user-data socket below its 103-byte path limit.
  const installRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tc-"));
  const shotsRoot = path.join(extensionRoot, "..", ".agents", "shots");
  await fs.mkdir(shotsRoot, { recursive: true });
  const artifactsRoot = process.env.TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR
    ? path.resolve(process.env.TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR)
    : await fs.mkdtemp(path.join(shotsRoot, "connectors-installed-"));
  await fs.mkdir(artifactsRoot, { recursive: true });
  console.log(`Real Serve connector acceptance artifacts: ${artifactsRoot}`);
  const extensionsDir = path.join(installRoot, "extensions");
  const vsixPath = path.join(artifactsRoot, "tomcat-vscode-ext.vsix");
  let fixture: Awaited<ReturnType<typeof startHttpFixture>> | undefined;
  const outcomes: Array<{ phase: string; status: string; error?: string }> = [];
  let failure: unknown;
  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], { cwd: extensionRoot, stdio: "inherit" });
    packageVsix({ extensionRoot, outPath: vsixPath, skipBuild: process.env.TOMCAT_ACCEPT_SKIP_BUILD === "1" });
    execFileSync(resolveVsCodeCli(), ["--user-data-dir", path.join(installRoot, "installer"), "--extensions-dir", extensionsDir, "--install-extension", vsixPath, "--force"], { stdio: "inherit" });
    const candidates = (await fs.readdir(extensionsDir, { withFileTypes: true })).filter((entry) => entry.isDirectory() && entry.name.startsWith("tomcat.tomcat-vscode-ext-"));
    if (candidates.length !== 1) throw new Error(`Expected one installed Tomcat extension, found ${candidates.length}`);
    const installed = path.join(extensionsDir, candidates[0].name);
    const expectedAssets = await assetHashes(path.join(extensionRoot, "gui/dist"));
    const installedAssets = await assetHashes(path.join(installed, "gui/dist"));
    for (const [asset, hash] of Object.entries(expectedAssets)) {
      if (installedAssets[asset] !== hash) throw new Error(`Installed GUI artifact differs: ${asset}`);
    }
    if (!expectedAssets["settings.js"]) throw new Error("Built Settings entry is missing");
    const extensionHash = await sha256(path.join(extensionRoot, "out/extension.js"));
    if (await sha256(path.join(installed, "out/extension.js")) !== extensionHash) throw new Error("Installed extension.js differs from this build");
    const cliHash = await sha256(executable);
    await fs.writeFile(path.join(artifactsRoot, "artifact-hashes.json"), JSON.stringify({
      executable, cliSha256: cliHash,
      cliVersion: execFileSync(executable, ["--version"], { env: outsideAgentEnv, encoding: "utf8" }).trim(),
      mcpBinary, mcpSha256: await sha256(mcpBinary), vsixPath, vsixSha256: await sha256(vsixPath),
      installedExtension: installed, extensionJsSha256: extensionHash,
      workspaceAssets: expectedAssets, installedAssets,
    }, null, 2));
    fixture = await startHttpFixture(mcpBinary, installRoot, artifactsRoot);
    // A separate clean profile follows the intentional browser-error probe.
    // Its expected console error cannot poison or excuse normal acceptance.
    for (const phase of ["checker-selftest", "normal", "untrusted", "add-and-trust"] as const) {
      const scenarioRoot = path.join(installRoot, { "checker-selftest": "s", normal: "n", untrusted: "u", "add-and-trust": "a" }[phase]);
      const userDataDir = path.join(scenarioRoot, "u");
      const workspaceDir = path.join(scenarioRoot, "w");
      const homeDir = path.join(scenarioRoot, "h");
      const artifacts = path.join(artifactsRoot, phase);
      await Promise.all([userDataDir, workspaceDir, homeDir, artifacts].map((directory) => fs.mkdir(directory, { recursive: true })));
      await fs.writeFile(path.join(workspaceDir, "README.md"), "# Isolated real connector acceptance\n");
      execFileSync(executable, ["init"], { cwd: workspaceDir, env: { ...outsideAgentEnv, HOME: homeDir }, stdio: "pipe" });
      await fs.writeFile(path.join(homeDir, ".tomcat", "mcp.json"), phase === "untrusted" || phase === "add-and-trust"
        ? `${JSON.stringify({ mcpServers: { "always-global": { url: `${fixture.url}/mcp`, auth: "none" } } })}\n`
        : '{"mcpServers":{}}\n');
      if (phase === "untrusted") {
        // Only this owned test workspace has project sources before approval.
        const generated = await fs.readFile(path.join(homeDir, ".tomcat", "tomcat.config.toml"), "utf8");
        const resourceDir = /^project_resource_dir\s*=\s*"([^"]+)"/m.exec(generated)?.[1] ?? ".agents";
        if (path.isAbsolute(resourceDir) || resourceDir.split(/[\\/]/).includes("..")) throw new Error("Unsafe test resource directory");
        const folder = path.join(workspaceDir, resourceDir);
        await fs.mkdir(folder, { recursive: true });
        const server = path.resolve(extensionRoot, "../tomcat/tests/fixtures/mcp/fake_stdio_server.mjs");
        await fs.access(server);
        await fs.writeFile(path.join(folder, "mcp.json"), `${JSON.stringify({ mcpServers: {
          "project-one": { command: "node", args: [server] },
          "project-two": { command: "node", args: [server] },
        } })}\n`);
      }
      await seedRealServeSettings(userDataDir, executable, workspaceDir);
      const priorAgentActive = process.env.TOMCAT_AGENT_ACTIVE;
      const priorKey = process.env.OPENAI_API_KEY;
      delete process.env.TOMCAT_AGENT_ACTIVE;
      process.env.OPENAI_API_KEY = "connector-acceptance-placeholder";
      try {
        await runVsCodeGuiTests(runTests, {
          extensionDevelopmentPath: harnessRoot,
          extensionTestsPath: path.join(harnessRoot, "out/test/connectors-installed-acceptance.index.js"),
          extensionTestsEnv: {
            ...outsideAgentEnv, HOME: homeDir, OPENAI_API_KEY: "connector-acceptance-placeholder",
            TOMCAT_CONNECTORS_ACCEPT_BINARY: executable,
            TOMCAT_VSCODE_TEST_PATH: executable, TOMCAT_VSCODE_TEST_EXTRA_ARGS: "",
            TOMCAT_CONNECTORS_ACCEPT_PHASE: phase,
            TOMCAT_CONNECTORS_ACCEPT_CHECKER_SELFTEST: phase === "checker-selftest" ? "1" : "0",
            TOMCAT_CONNECTORS_ACCEPT_MCP_URL: fixture.url,
            TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR: artifacts,
            TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifacts, TOMCAT_ACCEPT_SCREENSHOTS_DIR: artifacts,
            TOMCAT_E2E_SCREENSHOT: "1", TOMCAT_VSCODE_TEST_DEFAULT_CWD: workspaceDir,
            TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
          },
          // This owned, key-free test profile must not prompt for the OS keychain.
          launchArgs: [workspaceDir, `--extensions-dir=${extensionsDir}`, `--user-data-dir=${userDataDir}`, "--use-inmemory-secretstorage"],
          reuseMachineInstall: true, vscodeExecutablePath: resolveVsCodeExecutable(),
        });
        if (await sha256(executable) !== cliHash) throw new Error("CLI changed while acceptance was running");
        outcomes.push({ phase, status: "passed" });
      } catch (error) {
        outcomes.push({ phase, status: "failed", error: String(error) });
        throw error;
      } finally {
        if (priorAgentActive === undefined) delete process.env.TOMCAT_AGENT_ACTIVE; else process.env.TOMCAT_AGENT_ACTIVE = priorAgentActive;
        if (priorKey === undefined) delete process.env.OPENAI_API_KEY; else process.env.OPENAI_API_KEY = priorKey;
      }
    }
  } catch (error) { failure = error; throw error; }
  finally {
    try {
      await fixture?.stop();
      await fs.writeFile(path.join(artifactsRoot, "acceptance-result.json"), JSON.stringify({ outcomes, error: failure ? String(failure) : undefined }, null, 2));
    } finally { await fs.rm(installRoot, { force: true, recursive: true }); }
  }
}
main().catch((error) => { console.error(error); process.exitCode = 1; });
