import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

import { resolveVsCodeCli, resolveVsCodeExecutable, seedChatUserSettings } from "./e2eHostFixture";
import { packageVsix } from "./package-vsix";
import { runVsCodeGuiTests } from "./vscodeLaunchEnv";

const outsideAgentEnv: NodeJS.ProcessEnv = { ...process.env };
for (const key of Object.keys(outsideAgentEnv)) {
  // The acceptance host owns its test credentials and must not inherit a parent
  // agent session or real provider credential.
  if (key === "TOMCAT_AGENT_ACTIVE" || key.startsWith("TOMCAT__") || /_API_KEY$/i.test(key)) {
    outsideAgentEnv[key] = undefined;
  }
}
// A developer proxy can route VS Code's local webview/resource traffic through an
// unavailable SOCKS endpoint. This test only needs its loopback Serve process.
Object.assign(outsideAgentEnv, {
  ALL_PROXY: "",
  HTTPS_PROXY: "",
  HTTP_PROXY: "",
  NO_PROXY: "127.0.0.1,localhost",
  all_proxy: "",
  https_proxy: "",
  http_proxy: "",
  no_proxy: "127.0.0.1,localhost",
});

async function sha256(filePath: string): Promise<string> {
  return createHash("sha256").update(await fs.readFile(filePath)).digest("hex");
}

async function seedRealServeSettings(
  userDataDir: string,
  executable: string,
  workspaceDir: string,
): Promise<void> {
  await seedChatUserSettings(userDataDir);
  const settingsPath = path.join(userDataDir, "User", "settings.json");
  const current = JSON.parse(await fs.readFile(settingsPath, "utf8")) as Record<string, unknown>;
  await fs.writeFile(
    settingsPath,
    `${JSON.stringify({
      ...current,
      "extensions.autoCheckUpdates": false,
      "extensions.autoUpdate": "off",
      "security.workspace.trust.enabled": false,
      "telemetry.telemetryLevel": "off",
      "tomcat.path": executable,
      "tomcat.session.defaultCwd": workspaceDir,
      "update.mode": "none",
      "workbench.startupEditor": "none",
      "workbench.tips.enabled": false,
    }, null, 2)}\n`,
    "utf8",
  );
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.join(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.join(
    harnessRoot,
    "out/test/connectors-installed-acceptance.index.js",
  );
  const executable = path.resolve(
    process.env.TOMCAT_CONNECTORS_ACCEPT_BINARY
      ?? path.join(extensionRoot, "..", "tomcat", "target", "debug", "tomcat"),
  );
  await fs.access(executable);

  const installRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-real-connectors-"));
  const artifactsRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-real-connectors-artifacts-"));
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  const workspaceDir = path.join(installRoot, "workspace");
  const homeDir = path.join(installRoot, "home");
  const vsixPath = path.join(installRoot, "tomcat-vscode-ext.vsix");

  try {
    await Promise.all([
      fs.mkdir(extensionsDir, { recursive: true }),
      fs.mkdir(homeDir, { recursive: true }),
      fs.mkdir(userDataDir, { recursive: true }),
      fs.mkdir(workspaceDir, { recursive: true }),
    ]);
    await fs.writeFile(path.join(workspaceDir, "README.md"), "# Real connector acceptance\n", "utf8");
    await seedRealServeSettings(userDataDir, executable, workspaceDir);
    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    packageVsix({
      extensionRoot,
      outPath: vsixPath,
      skipBuild: process.env.TOMCAT_ACCEPT_SKIP_BUILD === "1",
    });
    execFileSync(resolveVsCodeCli(), [
      "--user-data-dir", userDataDir,
      "--extensions-dir", extensionsDir,
      "--install-extension", vsixPath,
      "--force",
    ], { stdio: "inherit" });
    await fs.access(harnessTestsPath);

    // @vscode/test-electron merges process.env with extensionTestsEnv. Clear the
    // parent marker for this owned subprocess as well as the supplied child env.
    const priorAgentActive = process.env.TOMCAT_AGENT_ACTIVE;
    const priorOpenAiKey = process.env.OPENAI_API_KEY;
    delete process.env.TOMCAT_AGENT_ACTIVE;
    process.env.OPENAI_API_KEY = "connector-acceptance-placeholder";
    try {
      await runVsCodeGuiTests(runTests, {
        extensionDevelopmentPath: harnessRoot,
        extensionTestsEnv: {
          ...outsideAgentEnv,
          HOME: homeDir,
          // The test never sends a model prompt, but Serve validates its configured
          // default provider before it accepts the connector-only control surface.
          OPENAI_API_KEY: "connector-acceptance-placeholder",
          TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR: artifactsRoot,
          TOMCAT_E2E_SCREENSHOT: "1",
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
    } finally {
      if (priorAgentActive === undefined) delete process.env.TOMCAT_AGENT_ACTIVE;
      else process.env.TOMCAT_AGENT_ACTIVE = priorAgentActive;
      if (priorOpenAiKey === undefined) delete process.env.OPENAI_API_KEY;
      else process.env.OPENAI_API_KEY = priorOpenAiKey;
    }
    await fs.writeFile(
      path.join(artifactsRoot, "artifact-hashes.json"),
      `${JSON.stringify({
        extensionJsSha256: await sha256(path.join(extensionRoot, "out", "extension.js")),
        settingsJsSha256: await sha256(path.join(extensionRoot, "gui", "dist", "settings.js")),
      }, null, 2)}\n`,
      "utf8",
    );
    console.log(`Real Serve connector acceptance artifacts: ${artifactsRoot}`);
  } finally {
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
