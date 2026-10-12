import { execFileSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { runTests } from "@vscode/test-electron";
import { runVsCodeGuiTests } from "./vscodeLaunchEnv";

import {
  resolveVsCodeCli,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";
import { packageVsix } from "./package-vsix";

async function seedAcceptanceSettings(
  userDataDir: string,
  fakeServePath: string,
): Promise<void> {
  await seedChatUserSettings(userDataDir);
  const settingsPath = path.join(userDataDir, "User", "settings.json");
  const current = JSON.parse(
    await fs.readFile(settingsPath, "utf8"),
  ) as Record<string, unknown>;
  await fs.writeFile(
    settingsPath,
    `${JSON.stringify(
      {
        ...current,
        "extensions.autoCheckUpdates": false,
        "extensions.autoUpdate": "off",
        "security.workspace.trust.enabled": false,
        "telemetry.telemetryLevel": "off",
        "tomcat.path": fakeServePath,
        "update.mode": "none",
        "window.commandCenter": false,
        "workbench.startupEditor": "none",
        "workbench.tips.enabled": false,
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
}

function artifactDirectory(extensionRoot: string): string {
  const requested = process.env.TOMCAT_IMAGE_ACCEPT_ARTIFACTS_DIR?.trim();
  if (requested) {
    return path.resolve(requested);
  }
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  return path.join(extensionRoot, "artifacts", "image-acceptance", stamp);
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.join(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.join(
    harnessRoot,
    "out",
    "test",
    "image-acceptance.index.js",
  );
  const installRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tia-"));
  const artifactsParent = artifactDirectory(extensionRoot);
  await fs.mkdir(artifactsParent, { recursive: true });
  const artifactsRoot = await fs.mkdtemp(path.join(artifactsParent, "run-"));
  const extensionsDir = path.join(installRoot, "extensions");
  const fakeServeStateDir = path.join(installRoot, "fake-serve-state");
  const userDataDir = path.join(installRoot, "user-data");
  const workspaceDir = path.join(installRoot, "workspace");
  const screenshotsDir = path.join(artifactsRoot, "screenshots");
  const reportPath = path.join(artifactsRoot, "image-acceptance-report.json");
  const vsixPath = path.join(installRoot, "tomcat-vscode-ext.vsix");
  const fakeServePath = path.join(
    extensionRoot,
    "scripts",
    "manual-acceptance",
    "fake-serve.js",
  );

  try {
    await Promise.all([
      fs.mkdir(extensionsDir, { recursive: true }),
      fs.mkdir(fakeServeStateDir, { recursive: true }),
      fs.mkdir(userDataDir, { recursive: true }),
      fs.mkdir(workspaceDir, { recursive: true }),
      fs.mkdir(screenshotsDir, { recursive: true }),
    ]);
    await fs.chmod(fakeServePath, 0o755);
    await fs.writeFile(
      path.join(workspaceDir, "README.md"),
      "# Tomcat image acceptance workspace\n",
      "utf8",
    );
    await seedAcceptanceSettings(userDataDir, fakeServePath);
    console.log(`Image acceptance artifacts will be written to: ${artifactsRoot}`);

    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    packageVsix({
      extensionRoot,
      outPath: vsixPath,
      skipBuild: process.env.TOMCAT_ACCEPT_SKIP_BUILD === "1",
    });
    execFileSync(
      resolveVsCodeCli(),
      [
        "--user-data-dir",
        userDataDir,
        "--extensions-dir",
        extensionsDir,
        "--install-extension",
        vsixPath,
        "--force",
      ],
      { stdio: "inherit" },
    );

    await fs.access(harnessTestsPath);
    {
      await runVsCodeGuiTests(runTests, {
        extensionDevelopmentPath: harnessRoot,
        extensionTestsEnv: {
          ...process.env,
          TOMCAT_ACCEPT_REPORT_PATH: reportPath,
          TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifactsRoot,
          TOMCAT_ACCEPT_SCREENSHOTS_DIR: screenshotsDir,
          TOMCAT_FAKE_SERVE_STATE_DIR: fakeServeStateDir,
          TOMCAT_VSCODE_TEST_DEFAULT_CWD: workspaceDir,
          TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
        },
        extensionTestsPath: harnessTestsPath,
        launchArgs: [
          workspaceDir,
          `--extensions-dir=${extensionsDir}`,
          `--user-data-dir=${userDataDir}`,
        ],
        reuseMachineInstall: true,
        vscodeExecutablePath: resolveVsCodeExecutable(),
      });
    }

    console.log(`Image acceptance artifacts: ${artifactsRoot}`);
    console.log(await fs.readFile(reportPath, "utf8"));
  } finally {
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
