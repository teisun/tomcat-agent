import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

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
    await runTests({
      extensionDevelopmentPath: options.harnessRoot,
      extensionTestsEnv: options.testEnv,
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
  // Artifacts live OUTSIDE installRoot so the finally cleanup retains them for
  // post-run inspection (Read cropped screenshots). Override via env if needed.
  const artifactsDir = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR
    ?? path.join(os.tmpdir(), "tomcat-vsix-verify-artifacts");
  const bundledFixture = await createHostE2eFixture();
  const setupRequiredFixture = await createHostE2eFixture({ requireInit: true });
  const prebuiltVsixRoot = await fs.mkdtemp("/tmp/tvsi-prebuilt-vsix-");
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredBundledTestPath,
    ...bundledOnlyFixtureEnv
  } = bundledFixture.env;
  const {
    TOMCAT_VSCODE_TEST_PATH: _ignoredSetupTestPath,
    ...setupRequiredBundledEnv
  } = setupRequiredFixture.env;

  await fs.mkdir(artifactsDir, { recursive: true });
  await clearVisualArtifacts(artifactsDir);

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
        "keeps cross-owner plan state in sync in the webview",
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
    TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
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
    TOMCAT_EXPECT_PROMPT_ACTIONS: "Start Setup|View Guide",
    TOMCAT_EXPECT_PROMPT_SEVERITY: "info",
    TOMCAT_EXPECT_PROMPT_SUBSTRING: "Tomcat is installed, but it is not ready yet",
    TOMCAT_EXPECT_RESOLVED_SOURCE: "bundled",
    TOMCAT_EXPECT_SETUP_RECOVERY: "1",
    TOMCAT_EXPECT_PROMPT_TRIGGER: "restart",
    TOMCAT_VSCODE_TEST_INFO_ACTION: "Start Setup",
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
      extensionRoot,
      harnessRoot,
      harnessTestsPath: promptOnlyHarnessTestsPath,
      testEnv: pureExtPromptEnv,
      vsixPath: reusableVsixPath,
      workspacePath: pureExtWorkspacePath,
    });

    await runInstalledScenario({
      extensionRoot,
      harnessRoot,
      harnessTestsPath: setupRecoveryHarnessTestsPath,
      testEnv: setupRecoveryEnv,
      vsixPath: reusableVsixPath,
      workspacePath: setupRequiredFixture.workspaceDir,
    });
  } finally {
    // Best-effort crop even if runTests rejected (partial screenshots may exist).
    await cropScreenshots(extensionRoot, artifactsDir);
    console.log(`\nverify:vsix artifacts (screenshots + crops): ${artifactsDir}`);
    await bundledFixture.cleanup();
    await setupRequiredFixture.cleanup();
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
    execFileSync(
      "python3",
      [cropper, "--artifacts-dir", artifactsDir],
      { stdio: "inherit" },
    );
  } catch (error) {
    console.warn(`crop-screenshot.py failed (screenshots may still be readable as full-frame): ${String(error)}`);
  }
}

async function clearVisualArtifacts(artifactsDir: string): Promise<void> {
  const entries = await fs.readdir(artifactsDir, { withFileTypes: true });
  await Promise.all(
    entries
      .filter((entry) => entry.isFile() && /^tomcat-vsix-visual-.*\.png$/u.test(entry.name))
      .map((entry) => fs.rm(path.join(artifactsDir, entry.name), { force: true })),
  );
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
