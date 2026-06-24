import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

import { resolveVsCodeCli, resolveVsCodeExecutable, seedChatUserSettings } from "./e2eHostFixture";
import { packageVsix } from "./package-vsix";

async function seedReverifySettings(
  userDataDir: string,
  fakeServePath: string,
): Promise<void> {
  await seedChatUserSettings(userDataDir);
  const settingsDir = path.join(userDataDir, "User");
  const settingsPath = path.join(settingsDir, "settings.json");
  const current = JSON.parse(await fs.readFile(settingsPath, "utf8")) as Record<string, unknown>;
  const merged = {
    ...current,
    "extensions.autoCheckUpdates": false,
    "extensions.autoUpdate": "off",
    "security.workspace.trust.enabled": false,
    "telemetry.telemetryLevel": "off",
    "tomcat.path": fakeServePath,
    "tomcat.ui": "webview",
    "update.mode": "none",
    "window.commandCenter": false,
    "workbench.startupEditor": "none",
    "workbench.tips.enabled": false,
  };
  await fs.writeFile(settingsPath, `${JSON.stringify(merged, null, 2)}\n`, "utf8");
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.resolve(
    harnessRoot,
    "out/test/model-switch-reverify.index.js",
  );
  const installRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tmrh-"));
  const artifactsRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tmra-"));
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  const workspaceDir = path.join(installRoot, "workspace");
  const screenshotsDir = path.join(artifactsRoot, "screenshots");
  const reportPath = path.join(artifactsRoot, "model-switch-reverify-report.json");
  const vsixPath = path.join(installRoot, "tomcat-vscode-ext.vsix");
  const fakeServePath = path.join(
    extensionRoot,
    "scripts",
    "manual-acceptance",
    "fake-serve.js",
  );

  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    await fs.mkdir(userDataDir, { recursive: true });
    await fs.mkdir(workspaceDir, { recursive: true });
    await fs.mkdir(screenshotsDir, { recursive: true });
    await fs.writeFile(
      path.join(workspaceDir, "README.md"),
      "# Model switch real host reverify workspace\n",
      "utf8",
    );
    await seedReverifySettings(userDataDir, fakeServePath);
    console.log(`Model switch reverify artifacts will be written to: ${artifactsRoot}`);

    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    packageVsix({ extensionRoot, outPath: vsixPath });
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
      {
        stdio: "inherit",
      },
    );

    await fs.access(harnessTestsPath);
    await runTests({
      extensionDevelopmentPath: harnessRoot,
      extensionTestsEnv: {
        ...process.env,
        TOMCAT_ACCEPT_REPORT_PATH: reportPath,
        TOMCAT_ACCEPT_SCREENSHOTS_DIR: screenshotsDir,
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

    const reportText = await fs.readFile(reportPath, "utf8");
    console.log(`Model switch reverify artifacts: ${artifactsRoot}`);
    console.log(reportText);
  } finally {
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
