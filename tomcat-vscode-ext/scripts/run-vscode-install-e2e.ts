import * as path from "node:path";
import * as fs from "node:fs/promises";
import {
  createHostE2eFixture,
  resolveVsCodeCli,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";
import { packageVsixOrReuse } from "./package-vsix";
import { runCommand } from "./runCommand";
import { reserveLoopbackPort, runVsCodeGuiTests } from "./vscodeLaunchEnv";
import { setupRealServeFixture } from "./realServeFixture";

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.resolve(harnessRoot, "out/test/index.js");
  const installRoot = await fs.mkdtemp("/tmp/tvsi-");
  const originalCwd = process.cwd();
  const { runTests } = await (async () => {
    process.chdir(installRoot);
    try { return await import("@vscode/test-electron"); }
    finally { process.chdir(originalCwd); }
  })();
  const extensionsDir = path.join(installRoot, ".vscode-test", "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  const realObservation = process.env.TOMCAT_E2E_REAL_OBSERVATION === "1";
  const fixture = await (async () => {
    if (!realObservation) return createHostE2eFixture();
    const binary = path.resolve(process.env.CARGO_TARGET_DIR ?? path.resolve(extensionRoot, "../tomcat/target"),
      "debug", process.platform === "win32" ? "tomcat.exe" : "tomcat");
    await fs.access(binary);
    const modelPort = await reserveLoopbackPort();
    const real = await setupRealServeFixture(`http://127.0.0.1:${modelPort}`, "openai", { binary });
    return { cleanup: real.cleanup, workspaceDir: real.workspacePath, env: {
      ...real.env, TOMCAT_VSCODE_TEST_PATH: binary, TOMCAT_VSCODE_TEST_DEFAULT_CWD: real.workspacePath,
      TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1", TOMCAT_E2E_OBSERVATION_MODEL_PORT: String(modelPort),
      TOMCAT_E2E_GREP: process.env.TOMCAT_E2E_GREP ?? "recovers an observed session",
    } };
  })();

  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    await fs.mkdir(userDataDir, { recursive: true });
    await seedChatUserSettings(userDataDir);

    runCommand("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], { cwd: extensionRoot });
    const packagedVsixPath = packageVsixOrReuse({
      extensionRoot,
      outPath: path.join(installRoot, "tomcat-vscode-ext.vsix"),
    });
    runCommand(resolveVsCodeCli(), [
      "--user-data-dir",
      userDataDir,
      "--extensions-dir",
      extensionsDir,
      "--install-extension",
      packagedVsixPath,
      "--force",
    ]);

    await fs.access(harnessTestsPath);
    await runVsCodeGuiTests(runTests, {
      extensionDevelopmentPath: harnessRoot,
      extensionTestsEnv: fixture.env,
      extensionTestsPath: harnessTestsPath,
      launchArgs: [
        `--user-data-dir=${userDataDir}`,
        `--extensions-dir=${extensionsDir}`,
        realObservation ? fixture.workspaceDir : path.resolve(extensionRoot, ".."),
      ],
      reuseMachineInstall: false,
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
  } finally {
    await fixture.cleanup();
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
