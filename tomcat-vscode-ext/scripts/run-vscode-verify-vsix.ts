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
import { packageVsix } from "./package-vsix";

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.resolve(harnessRoot, "out/test/index.js");
  const installRoot = await fs.mkdtemp("/tmp/tvsi-verify-");
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  const vsixPath = path.join(installRoot, "tomcat-vscode-ext.vsix");
  // Artifacts live OUTSIDE installRoot so the finally cleanup retains them for
  // post-run inspection (Read cropped screenshots). Override via env if needed.
  const artifactsDir = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR
    ?? path.join(os.tmpdir(), "tomcat-vsix-verify-artifacts");
  const fixture = await createHostE2eFixture();

  await fs.mkdir(artifactsDir, { recursive: true });
  await clearVisualArtifacts(artifactsDir);

  const verifyEnv: NodeJS.ProcessEnv = {
    ...process.env,
    ...fixture.env,
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
    TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifactsDir,
  };

  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    await fs.mkdir(userDataDir, { recursive: true });
    await seedChatUserSettings(userDataDir);

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
      extensionTestsEnv: verifyEnv,
      extensionTestsPath: harnessTestsPath,
      launchArgs: [
        path.resolve(extensionRoot, ".."),
        `--extensions-dir=${extensionsDir}`,
        `--user-data-dir=${userDataDir}`,
      ],
      reuseMachineInstall: true,
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
  } finally {
    // Best-effort crop even if runTests rejected (partial screenshots may exist).
    await cropScreenshots(extensionRoot, artifactsDir);
    console.log(`\nverify:vsix artifacts (screenshots + crops): ${artifactsDir}`);
    await fixture.cleanup();
    await fs.rm(installRoot, { force: true, recursive: true });
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
