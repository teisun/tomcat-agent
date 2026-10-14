import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { execFileSync } from "node:child_process";

import { runTests, downloadAndUnzipVSCode, resolveCliPathFromVSCodeExecutablePath } from "@vscode/test-electron";
import { runVsCodeGuiTests } from "./vscodeLaunchEnv";

import {
  createHostE2eFixture,
  resolveVsCodeCli,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";
import { packageVsixOrReuse } from "./package-vsix";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { EXTENSION_ID } from "../src/constants";
import { extractVsixLikeCursor } from "./vsix-extractable";
import { setupRealServeFixture, spawnScriptedOpenAiStreamServer, type ScriptedResponse, sseDelta, sseFinish, sseDone, sseToolCall } from "./realServeFixture";

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

export type ProductionSource = { kind: "bundle" | "marketplace"; value: string };
export function parseProductionSource(args: string[]): ProductionSource | undefined {
  if (!args.length) return undefined;
  if (args.length !== 2 || !["--release-bundle", "--marketplace"].includes(args[0]) || !args[1]) {
    throw new Error("Use either --release-bundle <existing VSIX> or --marketplace <ID@version>");
  }
  if (args[0] === "--marketplace" && !new RegExp(`^${EXTENSION_ID.replace(/\./g, "\\.")}@[0-9]+\\.[0-9]+\\.[0-9]+$`).test(args[1])) {
    throw new Error(`Marketplace source must be ${EXTENSION_ID}@<exact version>`);
  }
  return { kind: args[0] === "--release-bundle" ? "bundle" : "marketplace", value: args[1] };
}
type ReleaseMetadata = { publisher: string; name: string; version: string; tomcat: { bundledCliVersion: string }; engines: { vscode: string } };
function expectedRelease(): ReleaseMetadata {
  return JSON.parse(readFileSync(path.resolve(__dirname, "../package.json"), "utf8"));
}
export function validateReleaseMetadata(metadata: ReleaseMetadata, target: string, actualTarget: string): void {
  const expected = expectedRelease();
  if (`${metadata.publisher}.${metadata.name}` !== EXTENSION_ID || metadata.version !== expected.version || metadata.tomcat?.bundledCliVersion !== expected.tomcat.bundledCliVersion) {
    throw new Error("Release bundle identity/version/CLI pin does not match this release");
  }
  if (metadata.engines?.vscode !== expected.engines.vscode) throw new Error("Release bundle VS Code engine does not match this release");
  if (actualTarget !== target) throw new Error(`Release bundle target mismatch: expected ${target}, got ${actualTarget}`);
}
async function inspectReleaseVsix(file: string) {
  await fs.access(file); // A missing filename must never turn into a gallery ID.
  await extractVsixLikeCursor(file);
  const metadata = JSON.parse(execFileSync("unzip", ["-p", file, "extension/package.json"], { encoding: "utf8" })) as ReleaseMetadata;
  const xml = execFileSync("unzip", ["-p", file, "extension.vsixmanifest"], { encoding: "utf8" });
  const target = /TargetPlatform="([^"]+)"/.exec(xml)?.[1] ?? "universal";
  const identity = /<Identity\s+([^>]+)\/>/.exec(xml)?.[1];
  for (const [key, value] of Object.entries({ Id: metadata.name, Publisher: metadata.publisher, Version: metadata.version })) {
    if (!identity || new RegExp(`${key}="([^"]+)"`).exec(identity)?.[1] !== value) throw new Error(`VSIX manifest ${key} does not match package metadata`);
  }
  validateReleaseMetadata(metadata, currentVsCodeTarget(), target);
  const binary = execFileSync("unzip", ["-p", file, "extension/bin/tomcat"], { maxBuffer: 128 * 1024 * 1024 });
  return { metadata, target, vsixSha256: createHash("sha256").update(await fs.readFile(file)).digest("hex"), cliSha256: createHash("sha256").update(binary).digest("hex") };
}

type InstalledScenarioOptions = {
  production?: { source: ProductionSource; bundle?: Awaited<ReturnType<typeof inspectReleaseVsix>> };
  artifactsDir: string;
  additionalSettings?: Record<string, unknown>;
  extensionRoot: string;
  harnessRoot: string;
  harnessTestsPath: string;
  testEnv: NodeJS.ProcessEnv;
  vsixPath: string;
  workspacePath: string;
};

export async function runInstalledScenario(options: InstalledScenarioOptions): Promise<void> {
  const installRoot = await fs.mkdtemp("/tmp/tvsi-verify-");
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  let fixture: Awaited<ReturnType<typeof setupRealServeFixture>> | undefined;
  let server: Awaited<ReturnType<typeof spawnScriptedOpenAiStreamServer>> | undefined;

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

    let testEnv = options.testEnv;
    let workspacePath = options.workspacePath;
    if (options.production) {
      const dirs = (await fs.readdir(extensionsDir)).filter(name => name.startsWith(`${EXTENSION_ID}-`));
      if (dirs.length !== 1) throw new Error(`Expected exactly one installed ${EXTENSION_ID}, found ${dirs.length}`);
      const installed = path.join(extensionsDir, dirs[0]);
      const metadata = JSON.parse(await fs.readFile(path.join(installed, "package.json"), "utf8"));
      const profile: Array<{ identifier: { id: string }; version: string; metadata?: { source?: string; targetPlatform?: string } }> = JSON.parse(await fs.readFile(path.join(extensionsDir, "extensions.json"), "utf8"));
      const entries = profile.filter(entry => entry.identifier.id === EXTENSION_ID && entry.version === metadata.version);
      const source = options.production.source.kind === "bundle" ? "vsix" : "gallery";
      if (entries.length !== 1 || entries[0].metadata?.source !== source) throw new Error(`Installed extension source must be ${source}`);
      // VS Code records gallery metadata in extensions.json; local VSIX target
      // comes from the archive already checked before installation.
      validateReleaseMetadata(metadata, currentVsCodeTarget(), options.production.bundle?.target ?? entries[0].metadata?.targetPlatform ?? "universal");
      const binary = path.join(installed, "bin/tomcat");
      const cliSha256 = createHash("sha256").update(await fs.readFile(binary)).digest("hex");
      if (options.production.bundle && cliSha256 !== options.production.bundle.cliSha256) throw new Error("Installed CLI bytes differ from the validated VSIX");
      await fs.chmod(binary, 0o755);
      const binaryArchitecture = execFileSync("file", [binary], { encoding: "utf8" }).trim();
      const archPattern = process.arch === "arm64" ? /arm64|aarch64/ : /x86_64|x86-64/;
      const nativeMachine = execFileSync("uname", ["-m"], { encoding: "utf8" }).trim();
      if (!archPattern.test(binaryArchitecture) || !archPattern.test(nativeMachine)) throw new Error(`Not a native ${currentVsCodeTarget()} binary/host: ${binaryArchitecture}; ${nativeMachine}`);
      // Real CLI and Host; only upstream model replies are controlled.
      const responses: ScriptedResponse[] = [];
      server = await spawnScriptedOpenAiStreamServer(responses);
      fixture = await setupRealServeFixture(server.baseUrl, "openai", { binary, coldStart: true });
      // Tool paths are explicit fixture-owned absolute paths. Missing relative
      // files cannot be canonicalized into workspace_roots by the real gate.
      responses.push(
        { parts: [sseDelta("RELEASE_BUNDLE_READY"), sseFinish("stop"), sseDone()] },
        { parts: [sseToolCall("release-allow", "write", JSON.stringify({ path: path.join(fixture.workspacePath, "release-allowed.txt"), content: "allowed by workspace policy" })), sseFinish("tool_calls"), sseDone()] },
        { parts: [sseDelta("RELEASE_ALLOWED"), sseFinish("stop"), sseDone()] },
        { parts: [sseToolCall("release-deny", "write", JSON.stringify({ path: path.join(fixture.workspacePath, "release-denied.txt"), content: "must not be written" })), sseFinish("tool_calls"), sseDone()] },
        { parts: [sseDelta("RELEASE_DENIED"), sseFinish("stop"), sseDone()] },
        { parts: [sseDelta("RELEASE_STOP_STARTED"), { ...sseDelta("must not finish before stop"), delayMs: 60_000 }, sseFinish("stop"), sseDone()] },
        { parts: [sseDelta("RELEASE_RELOADED"), sseFinish("stop"), sseDone()] },
      );
      for (const name of ["tomcat.config.toml", "models.toml", ".env"]) {
        try { await fs.access(path.join(fixture.homePath, ".tomcat", name)); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") continue; throw error; }
        throw new Error(`Cold-start fixture unexpectedly contains ${name}`);
      }
      testEnv = { ...fixture.env };
      testEnv.TOMCAT_ONBOARDING_BASE_URL = server.baseUrl;
      for (const key of Object.keys(testEnv)) if (key.startsWith("TOMCAT_VSCODE_TEST_") || key.startsWith("TOMCAT_E2E_")) testEnv[key] = undefined;
      Object.assign(testEnv, { TOMCAT_VSCODE_TEST_DEFAULT_CWD: fixture.workspacePath, TOMCAT_EXPECT_RELEASE_BINARY: binary, TOMCAT_EXPECT_RELEASE_HOST_ARCH: process.arch, TOMCAT_EXPECT_RELEASE_CLI_SHA256: cliSha256, TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1" });
      const cliVersion = execFileSync(binary, ["--version"], { env: testEnv, encoding: "utf8" }).trim();
      if (cliVersion !== `tomcat ${metadata.tomcat.bundledCliVersion}`) throw new Error(`Bundled CLI version mismatch: ${cliVersion}`);
      workspacePath = fixture.workspacePath;
      const settingsPath = path.join(userDataDir, "User/settings.json");
      const settings = JSON.parse(await fs.readFile(settingsPath, "utf8"));
      Object.assign(settings, { "security.workspace.trust.enabled": false, "telemetry.telemetryLevel": "off", "extensions.autoUpdate": false, "extensions.autoCheckUpdates": false, "update.mode": "none" });
      delete settings["tomcat.path"];
      await fs.writeFile(settingsPath, JSON.stringify(settings));
      await fs.writeFile(path.join(options.artifactsDir, "release-artifact.json"), JSON.stringify({ source: options.production.source, ...options.production.bundle, installed, cliSha256, cliVersion, binaryArchitecture, nativeMachine, nodeArch: process.arch, platform: process.platform, osRelease: os.release() }, null, 2));
    }

    await fs.access(options.harnessTestsPath);
    await runVsCodeGuiTests(runTests, {
      extensionDevelopmentPath: options.harnessRoot,
      extensionTestsEnv: { ...testEnv, TOMCAT_E2E_SCREENSHOT: "1", TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: options.artifactsDir },
      extensionTestsPath: options.harnessTestsPath,
      launchArgs: [
        workspacePath,
        `--extensions-dir=${extensionsDir}`,
        `--user-data-dir=${userDataDir}`,
      ],
      reuseMachineInstall: true,
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
    if (server) {
      const requests = server.capturedNonTitleRequests();
      if (requests.length !== 7) throw new Error(`Expected seven real model requests, got ${requests.length}`);
      const models = requests.map(request => JSON.parse(request.split("\r\n\r\n")[1]).model);
      if (models.some(model => model !== "deepseek-v4-flash")) throw new Error(`Unexpected selected models: ${JSON.stringify(models)}`);
      await fs.writeFile(path.join(options.artifactsDir, "release-model-requests.json"), JSON.stringify({ count: requests.length, models }, null, 2));
    }
  } finally {
    try { await server?.close(); } finally {
      try { await fixture?.cleanup(); } finally {
        await fs.rm(installRoot, { force: true, recursive: true });
      }
    }
  }
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const productionSource = parseProductionSource(process.argv.slice(2));
  if (productionSource) {
    const bundle = productionSource.kind === "bundle" ? await inspectReleaseVsix(path.resolve(productionSource.value)) : undefined;
    if (productionSource.kind === "marketplace" && productionSource.value.split("@")[1] !== expectedRelease().version) throw new Error("Marketplace version must match this release");
    if (!process.env.VSCODE_EXECUTABLE_PATH) {
      const version = expectedRelease().engines.vscode.replace(/^\^/, "");
      if (!/^[0-9]+\.[0-9]+\.[0-9]+$/.test(version)) throw new Error(`Cannot resolve minimum VS Code version: ${version}`);
      const executable = await downloadAndUnzipVSCode({ version });
      process.env.VSCODE_EXECUTABLE_PATH = executable;
      process.env.VSCODE_CLI_PATH = resolveCliPathFromVSCodeExecutablePath(executable);
    }
    const artifactsDir = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR ?? path.join(extensionRoot, "../.agents/marketplace", expectedRelease().version, currentVsCodeTarget());
    await fs.mkdir(artifactsDir, { recursive: true });
    execFileSync("npx", ["--no-install", "tsc", "-p", "e2e-harness/tsconfig.json"], { cwd: extensionRoot, stdio: "inherit" });
    await runInstalledScenario({ extensionRoot, harnessRoot, artifactsDir, harnessTestsPath: path.join(harnessRoot, "out/test/release-bundle.index.js"), testEnv: {}, workspacePath: os.tmpdir(), vsixPath: productionSource.kind === "bundle" ? path.resolve(productionSource.value) : productionSource.value, production: { source: productionSource, bundle } });
    console.log(`Production bundle evidence: ${artifactsDir}`);
    return;
  }
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
      process.env.TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS ?? "4000",
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
    TOMCAT_EXPECT_PROMPT_SUBSTRING: "Tomcat CLI not found.",
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
      "recovers inline from a repaired startup failure without running init",
    ].join("|"),
    TOMCAT_EXPECT_PROMPT_ACTIONS: "Retry|View Logs",
    TOMCAT_EXPECT_PROMPT_DETAIL: "fake serve startup blocked until fixture repair",
    TOMCAT_EXPECT_PROMPT_SEVERITY: "warning",
    TOMCAT_EXPECT_PROMPT_SUBSTRING: "Tomcat failed to start",
    TOMCAT_EXPECT_RESOLVED_SOURCE: "bundled",
    TOMCAT_EXPECT_SETUP_RECOVERY: "1",
    TOMCAT_EXPECT_PROMPT_TRIGGER: "restart",
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

if (path.basename(process.argv[1] ?? "") === "run-vscode-verify-vsix.ts") {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
