import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";
import { runVsCodeGuiTests } from "./vscodeLaunchEnv";

import {
  createHostE2eFixture,
  resolveVsCodeCli,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";
import { packageVsixOrReuse } from "./package-vsix";

function currentVsCodeTarget(): string {
  if (process.platform === "darwin" && process.arch === "arm64") {
    return "darwin-arm64";
  }
  if (process.platform === "darwin" && process.arch === "x64") {
    return "darwin-x64";
  }
  if (process.platform === "linux" && process.arch === "x64") {
    return "linux-x64";
  }
  throw new Error(`Unsupported local verify platform: ${process.platform}/${process.arch}`);
}

type InstalledScenarioOptions = {
  artifactsDir: string;
  additionalSettings?: Record<string, unknown>;
  extensionRoot: string;
  harnessRoot: string;
  harnessTestsPath: string;
  testEnv: NodeJS.ProcessEnv;
  vsixPath: string;
  workspacePath: string;
};

async function runInstalledScenario(options: InstalledScenarioOptions): Promise<void> {
  const installRoot = await fs.mkdtemp("/tmp/tvsi-verify-");
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");

  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    await fs.mkdir(userDataDir, { recursive: true });
    await seedChatUserSettings(userDataDir);
    if (options.additionalSettings && Object.keys(options.additionalSettings).length > 0) {
      const settingsPath = path.join(userDataDir, "User", "settings.json");
      const currentSettings = JSON.parse(await fs.readFile(settingsPath, "utf8")) as Record<string, unknown>;
      await fs.writeFile(
        settingsPath,
        `${JSON.stringify({ ...currentSettings, ...options.additionalSettings }, null, 2)}\n`,
        "utf8",
      );
    }

    execFileSync(
      resolveVsCodeCli(),
      [
        "--user-data-dir",
        userDataDir,
        "--extensions-dir",
        extensionsDir,
        "--install-extension",
        options.vsixPath,
        "--force",
      ],
      {
        stdio: "inherit",
      },
    );

    await fs.access(options.harnessTestsPath);
    await runVsCodeGuiTests(runTests, {
      extensionDevelopmentPath: options.harnessRoot,
      extensionTestsEnv: { ...options.testEnv, TOMCAT_E2E_SCREENSHOT: "1", TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: options.artifactsDir },
      extensionTestsPath: options.harnessTestsPath,
      launchArgs: [
        options.workspacePath,
        `--extensions-dir=${extensionsDir}`,
        `--user-data-dir=${userDataDir}`,
      ],
      reuseMachineInstall: true,
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
  } finally {
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const bundledHarnessTestsPath = path.resolve(harnessRoot, "out/test/index.js");
  const promptOnlyHarnessTestsPath = path.resolve(harnessRoot, "out/test/prompt-only.index.js");
  const setupRecoveryHarnessTestsPath = path.resolve(harnessRoot, "out/test/setup-recovery.index.js");
  const transientRecoveryHarnessTestsPath = path.resolve(
    harnessRoot,
    "out/test/transient-recovery.index.js",
  );
  const slowHandshakeHarnessTestsPath = path.resolve(
    harnessRoot,
    "out/test/slow-handshake.index.js",
  );
  // Artifacts live OUTSIDE installRoot so the finally cleanup retains them for
  // post-run inspection (Read cropped screenshots). Override via env if needed.
  const artifactsParent = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR
    ?? path.join(os.tmpdir(), "tomcat-vsix-verify-artifacts");
  await fs.mkdir(artifactsParent, { recursive: true });
  const artifactsDir = await fs.mkdtemp(path.join(artifactsParent, "run-"));
  const bundledFixture = await createHostE2eFixture();
  const setupRequiredFixture = await createHostE2eFixture({ requireInit: true });
  const transientFailureFixture = await createHostE2eFixture({
    transientServeFailures: 1,
  });
  const slowHandshakeFixture = await createHostE2eFixture({
    handshakeDelayMs: 15_000,
  });
  const prebuiltVsixRoot = await fs.mkdtemp("/tmp/tvsi-prebuilt-vsix-");
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredBundledTestPath,
    ...bundledOnlyFixtureEnv
  } = bundledFixture.env;
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredSetupTestPath,
    ...setupRequiredBundledEnv
  } = setupRequiredFixture.env;
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredTransientTestPath,
    ...transientFailureBundledEnv
  } = transientFailureFixture.env;
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredSlowHandshakeTestPath,
    ...slowHandshakeBundledEnv
  } = slowHandshakeFixture.env;


  const bundledVerifyEnv: NodeJS.ProcessEnv = {
    ...process.env,
    ...bundledOnlyFixtureEnv,
    PATH: process.platform === "win32" ? process.env.PATH : "/usr/bin:/bin",
    TOMCAT_E2E_SCREENSHOT: "1",
    TOMCAT_E2E_CAPTURE_PROGRESS: process.env.TOMCAT_E2E_CAPTURE_PROGRESS ?? "1",
    TOMCAT_E2E_GREP:
      process.env.TOMCAT_E2E_GREP
      ?? [
        "restores plan cards and Ctx after switching sessions",
        "replays plan history after a webview reload",
        "switches an executing plan back to chat in the webview",
        "renders the transcript UI groups, tool rows, file chips, and progress",
      ].join("|"),
    TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS:
      process.env.TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS ?? "1500",
    TOMCAT_EXPECT_RESOLVED_SOURCE: "bundled",
    TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifactsDir,
  };
  const pureExtWorkspacePath = await fs.mkdtemp("/tmp/tvsi-pure-ext-workspace-");
  const pureExtPromptEnv: NodeJS.ProcessEnv = {
    ...process.env,
    PATH: process.platform === "win32" ? process.env.PATH : "/usr/bin:/bin",
    TOMCAT_E2E_GREP: "shows the expected onboarding prompt when requested by the host fixture",
    TOMCAT_EXPECT_PROMPT_ACTIONS: "View Guide|Open Settings",
    TOMCAT_EXPECT_PROMPT_SEVERITY: "warning",
    TOMCAT_EXPECT_PROMPT_SUBSTRING: "Tomcat CLI was not found automatically.",
    TOMCAT_VSCODE_TEST_DEFAULT_CWD: pureExtWorkspacePath,
    TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "0",
    TOMCAT_VSCODE_TEST_WARNING_ACTION: "",
    TOMCAT_VSCODE_TEST_INFO_ACTION: "",
  };
  const setupRecoveryEnv: NodeJS.ProcessEnv = {
    ...process.env,
    ...setupRequiredBundledEnv,
    PATH: process.platform === "win32" ? process.env.PATH : "/usr/bin:/bin",
    TOMCAT_E2E_GREP: [
      "uses the expected executable source when the host fixture asks for it",
      "shows the expected onboarding prompt when requested by the host fixture",
      "recovers from a setup-required startup when the test fixture auto-runs init",
    ].join("|"),
    TOMCAT_EXPECT_PROMPT_ACTIONS: "Start Setup|Open Settings|View Guide|Retry|View Logs",
    TOMCAT_EXPECT_PROMPT_DETAIL: "fake serve requires tomcat init first",
    TOMCAT_EXPECT_PROMPT_SEVERITY: "warning",
    TOMCAT_EXPECT_PROMPT_SUBSTRING: "Tomcat could not start after several attempts.",
    TOMCAT_EXPECT_RESOLVED_SOURCE: "bundled",
    TOMCAT_EXPECT_SETUP_RECOVERY: "1",
    TOMCAT_EXPECT_PROMPT_TRIGGER: "restart",
    TOMCAT_EXPECT_VISIBLE_SETUP: "1",
    TOMCAT_VSCODE_TEST_WARNING_ACTION: "",
    TOMCAT_VSCODE_TEST_INFO_ACTION: "",
    TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "0",
  };
  const transientRecoveryEnv: NodeJS.ProcessEnv = {
    ...process.env,
    ...transientFailureBundledEnv,
    PATH: process.platform === "win32" ? process.env.PATH : "/usr/bin:/bin",
    TOMCAT_EXPECT_TRANSIENT_SERVE_RECOVERY: "1",
    TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
  };
  const slowHandshakeEnv: NodeJS.ProcessEnv = {
    ...process.env,
    ...slowHandshakeBundledEnv,
    PATH: process.platform === "win32" ? process.env.PATH : "/usr/bin:/bin",
    TOMCAT_EXPECT_SLOW_HANDSHAKE: "1",
    TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
  };

  try {
    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    const reusableVsixPath = packageVsixOrReuse({
      bundleBinaryPath: bundledFixture.fakeServePath,
      extensionRoot,
      outPath: path.join(prebuiltVsixRoot, "tomcat-vscode-ext.vsix"),
      target: currentVsCodeTarget(),
    });

    await runInstalledScenario({
      artifactsDir,
      extensionRoot,
      harnessRoot,
      harnessTestsPath: bundledHarnessTestsPath,
      testEnv: bundledVerifyEnv,
      vsixPath: reusableVsixPath,
      workspacePath: path.resolve(extensionRoot, ".."),
    });

    await runInstalledScenario({
      additionalSettings: {
        "tomcat.path": path.join(pureExtWorkspacePath, "definitely-missing-tomcat"),
      },
      artifactsDir,
      extensionRoot,
      harnessRoot,
      harnessTestsPath: promptOnlyHarnessTestsPath,
      testEnv: pureExtPromptEnv,
      vsixPath: reusableVsixPath,
      workspacePath: pureExtWorkspacePath,
    });

    await runInstalledScenario({
      artifactsDir,
      extensionRoot,
      harnessRoot,
      harnessTestsPath: setupRecoveryHarnessTestsPath,
      testEnv: setupRecoveryEnv,
      vsixPath: reusableVsixPath,
      workspacePath: setupRequiredFixture.workspaceDir,
    });

    await runInstalledScenario({
      artifactsDir,
      extensionRoot,
      harnessRoot,
      harnessTestsPath: transientRecoveryHarnessTestsPath,
      testEnv: transientRecoveryEnv,
      vsixPath: reusableVsixPath,
      workspacePath: transientFailureFixture.workspaceDir,
    });

    await runInstalledScenario({
      artifactsDir,
      extensionRoot,
      harnessRoot,
      harnessTestsPath: slowHandshakeHarnessTestsPath,
      testEnv: slowHandshakeEnv,
      vsixPath: reusableVsixPath,
      workspacePath: slowHandshakeFixture.workspaceDir,
    });
  } finally {
    // Best-effort crop even if runTests rejected (partial screenshots may exist).
    await cropScreenshots(extensionRoot, artifactsDir);
    console.log(`\nverify:vsix artifacts (screenshots + crops): ${artifactsDir}`);
    await bundledFixture.cleanup();
    await setupRequiredFixture.cleanup();
    await transientFailureFixture.cleanup();
    await slowHandshakeFixture.cleanup();
    await fs.rm(prebuiltVsixRoot, { force: true, recursive: true });
    await fs.rm(pureExtWorkspacePath, { force: true, recursive: true });
  }
}

async function cropScreenshots(extensionRoot: string, artifactsDir: string): Promise<void> {
  const cropper = path.resolve(extensionRoot, "scripts/crop-screenshot.py");
  try {
    await fs.access(cropper);
  } catch {
    console.warn(`crop-screenshot.py not found at ${cropper}; skipping crop step`);
    return;
  }
  try {
    const runs = await fs.readdir(artifactsDir, { withFileTypes: true });
    for (const run of runs.filter((entry) => entry.isDirectory() && entry.name.startsWith("gui-"))) {
      const runDir = path.join(artifactsDir, run.name);
      const images = await fs.readdir(runDir);
      if (!images.some((name) => name.startsWith("tomcat-vsix-visual-") && name.endsWith(".png"))) continue;
      execFileSync("python3", [cropper, "--artifacts-dir", runDir], { stdio: "inherit" });
    }
  } catch (error) {
    console.warn(`crop-screenshot.py failed (screenshots may still be readable as full-frame): ${String(error)}`);
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
