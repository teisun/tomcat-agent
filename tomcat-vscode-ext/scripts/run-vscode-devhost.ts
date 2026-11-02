import * as fs from "node:fs/promises";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { downloadAndUnzipVSCode, runTests } from "@vscode/test-electron";
import { runCommand } from "./runCommand";
import { minimumVsCodeVersion, runVsCodeGuiTests, windowTestProxy } from "./vscodeLaunchEnv";
import { setupRealServeFixture, serveFixtureEnvironment } from "./realServeFixture";
import { setupSharedServiceLifecycle } from "./sharedServiceLifecycle";
import { readExternalObservationFixture } from "../src/test/suite/support/externalObservationFixture";

import {
  createHostE2eFixture,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";

async function reserveLoopbackPort(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  if (port <= 0) {
    throw new Error("Failed to reserve a loopback port for the VS Code workbench driver");
  }
  return port;
}

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, "..");
  const extensionTestsPath = path.resolve(
    extensionDevelopmentPath,
    "out/test/suite/index.js",
  );
  const transientServeFailures = Math.max(
    0,
    Number(process.env.TOMCAT_VSCODE_TEST_TRANSIENT_SERVE_FAILURES ?? "0") || 0,
  );
  const bootstrapListModelsFailures = Math.max(
    0,
    Number(process.env.TOMCAT_VSCODE_TEST_BOOTSTRAP_LIST_MODELS_FAILURES ?? "0") || 0,
  );
  const handshakeDelayMs = Math.max(
    0,
    Number(process.env.TOMCAT_VSCODE_TEST_HANDSHAKE_DELAY_MS ?? "0") || 0,
  );
  const requestedInterruptDelayMs = Number(
    process.env.TOMCAT_VSCODE_TEST_INTERRUPT_DELAY_MS ?? "150",
  );
  const interruptDelayMs = Number.isFinite(requestedInterruptDelayMs)
    ? Math.max(0, requestedInterruptDelayMs)
    : 150;
  const realObservation = process.env.TOMCAT_E2E_REAL_OBSERVATION === "1";
  const sharedLifecycle = process.env.TOMCAT_E2E_SHARED_LIFECYCLE === "1";
  const versionMismatch = process.env.TOMCAT_E2E_VERSION_MISMATCH === "1";
  const mismatchServiceBinary = process.env.TOMCAT_E2E_SERVICE_BINARY;
  if (mismatchServiceBinary && !versionMismatch) throw new Error("A second service binary requires the version-mismatch scenario");
  const externalObservation = process.env.TOMCAT_E2E_EXTERNAL_OBSERVATION;
  const llmProxy = windowTestProxy(process.env);
  if (llmProxy !== undefined && !realObservation && !versionMismatch) {
    throw new Error("Window proxy tests require an isolated real-service fixture, not a fake or external host");
  }
  if ([realObservation, Boolean(externalObservation), versionMismatch, sharedLifecycle].filter(Boolean).length > 1) {
    throw new Error("Choose only one real-service journey");
  }
  const fixture = await (async () => {
    if (externalObservation) {
      const external = await readExternalObservationFixture(externalObservation);
      await fs.access(external.binary);
      await fs.access(external.workspacePath);
      return {
        cleanup: async () => {}, // The host journey, not this observer window, owns service cleanup.
        workspaceDir: external.workspacePath,
        env: { ...serveFixtureEnvironment(external.homePath), TOMCAT_VSCODE_TEST_PATH: external.binary,
          TOMCAT_VSCODE_TEST_DEFAULT_CWD: external.workspacePath, TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1" },
      };
    }
    if (!realObservation && !versionMismatch && !sharedLifecycle) return createHostE2eFixture({ bootstrapListModelsFailures, handshakeDelayMs, interruptDelayMs, transientServeFailures });
    const binary = path.resolve(process.env.CARGO_TARGET_DIR ?? path.resolve(extensionDevelopmentPath, "../tomcat/target"), "debug", process.platform === "win32" ? "tomcat.exe" : "tomcat");
    await fs.access(binary); // Build the current CLI before this real-service journey.
    if (sharedLifecycle) return setupSharedServiceLifecycle(binary, process.env.TOMCAT_E2E_SYSTEM_PROXY === "1");
    const modelPort = await reserveLoopbackPort();
    if (versionMismatch && !mismatchServiceBinary && process.platform === "win32") {
      throw new Error("The version-mismatch visual fixture requires a POSIX wrapper");
    }
    const real = await setupRealServeFixture(`http://127.0.0.1:${modelPort}`, "openai", { binary, llmProxy });
    let executable = binary;
    if (versionMismatch && mismatchServiceBinary) {
      // Start a genuinely different executable; never rewrite its reported version.
      // The extension still uses the current CLI and the normal restart operation.
      try {
        await fs.access(mismatchServiceBinary);
        await promisify(execFile)(mismatchServiceBinary, ["serve", "start"], {
          cwd: real.workspacePath, env: real.env, timeout: 60_000,
        });
      } catch (error) {
        await real.cleanup();
        throw error;
      }
    } else if (versionMismatch) {
      // Fault-inject metadata only; restart and the recovered IPC connection use Rust.
      // This opt-in visual fixture uses a POSIX executable wrapper, not a Windows pass.
      executable = path.join(real.homePath, "tomcat-version-fixture");
      await fs.writeFile(executable, `#!/usr/bin/env node
const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const args = process.argv.slice(2);
const marker = __filename + ".restarted";
const result = spawnSync(${JSON.stringify(binary)}, args, { env: process.env, encoding: "utf8", stdio: ["inherit", "pipe", "inherit"] });
if (result.error) { console.error(result.error.message); process.exit(1); }
let output = result.stdout || "";
if (result.status === 0 && args[0] === "serve") {
  if (args[1] === "restart") fs.writeFileSync(marker, "restarted");
  if (["start", "status"].includes(args[1]) && !fs.existsSync(marker)) {
    const info = JSON.parse(output);
    if (info.running) info.serverVersion = "0.0.0-ui-fixture";
    output = JSON.stringify(info) + "\\n";
  }
}
process.stdout.write(output);
process.exit(result.status ?? 1);
`, { mode: 0o755 });
    }
    return {
      cleanup: real.cleanup,
      workspaceDir: real.workspacePath,
      env: { ...real.env, TOMCAT_E2E_OBSERVATION_MODEL_PORT: String(modelPort),
        TOMCAT_VSCODE_TEST_PATH: executable, TOMCAT_VSCODE_TEST_DEFAULT_CWD: real.workspacePath,
        TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1" },
    };
  })();
  const extensionTestsEnv: NodeJS.ProcessEnv = { ...fixture.env };
  if (realObservation) extensionTestsEnv.TOMCAT_E2E_GREP ??= "shared service observation";
  if (sharedLifecycle) extensionTestsEnv.TOMCAT_E2E_GREP ??= "shared service lifecycle";
  if (versionMismatch) extensionTestsEnv.TOMCAT_E2E_GREP ??= "shared service version mismatch";
  if (externalObservation) extensionTestsEnv.TOMCAT_E2E_GREP ??= "external host observation";
  for (const name of [
    "TOMCAT_E2E_GREP",
    "TOMCAT_E2E_EXTERNAL_OBSERVATION",
    "TOMCAT_E2E_VERSION_MISMATCH",
    "TOMCAT_E2E_SCREENSHOT",
    "TOMCAT_E2E_PLAN_FIND_CAPTURE_ONLY",
    "TOMCAT_EXPECT_SLOW_HANDSHAKE",
    "TOMCAT_EXPECT_TRANSIENT_SERVE_RECOVERY",
    "TOMCAT_VSCODE_TEST_BOOTSTRAP_LIST_MODELS_FAILURES",
    "TOMCAT_VSCODE_TEST_HANDSHAKE_DELAY_MS",
    "TOMCAT_VSCODE_TEST_INTERRUPT_DELAY_MS",
    "TOMCAT_VSCODE_TEST_TRANSIENT_SERVE_FAILURES",
    "TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR",
  ]) {
    const value = process.env[name];
    if (value) {
      extensionTestsEnv[name] = value;
    }
  }
  const userDataDir = await fs.mkdtemp(path.join(os.tmpdir(), "tdev-"));
  const cdpPort = await reserveLoopbackPort();

  try {
    runCommand("npm", ["run", "build"], { cwd: extensionDevelopmentPath });
    await seedChatUserSettings(userDataDir);
    // VS Code cannot enter a new workspace during extension tests. Start with a
    // one-root workspace so journeys can append/remove siblings without replacing A.
    const workspaceFile = path.join(userDataDir, "host.code-workspace");
    await fs.writeFile(workspaceFile, JSON.stringify({ folders: [{ path: fixture.workspaceDir }] }));

    await fs.access(extensionTestsPath);
    const vscodeExecutablePath = process.platform === "win32" && !process.env.VSCODE_EXECUTABLE_PATH
      ? await downloadAndUnzipVSCode({ version: await minimumVsCodeVersion() }) : resolveVsCodeExecutable();
    let artifacts: string | undefined;
    await runVsCodeGuiTests(async (options: Parameters<typeof runTests>[0]) => {
      artifacts = options.extensionTestsEnv?.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
      return runTests(options);
    }, {
      extensionDevelopmentPath,
      extensionTestsEnv: {
        ...extensionTestsEnv,
        TOMCAT_E2E_CDP_PORT: String(cdpPort),
      },
      extensionTestsPath,
      launchArgs: [
        `--remote-debugging-port=${cdpPort}`,
        `--user-data-dir=${userDataDir}`,
        // Search/drag/file fixtures belong to the disposable workspace, never the repository.
        workspaceFile,
      ],
      vscodeExecutablePath,
    });
    if ("verifyAfterWindowExit" in fixture) {
      if (!artifacts) throw new Error("Missing actual GUI artifact directory");
      try { await fixture.verifyAfterWindowExit(artifacts); }
      catch (error) {
        await fs.writeFile(path.join(artifacts, "window-exit-lifecycle.json"), JSON.stringify({ status: "failed", error: String(error) }, null, 2));
        throw error;
      }
    }
  } finally {
    try { await fixture.cleanup(); }
    finally { await fs.rm(userDataDir, { force: true, recursive: true, maxRetries: 3, retryDelay: 100 }); }
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
