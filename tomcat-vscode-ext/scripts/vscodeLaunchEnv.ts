import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { Writable } from "node:stream";
import * as net from "node:net";

// Existing manual/image acceptance keys, deliberately not a broad env purge.
export const GUI_MODE_KEYS = [
  "ELECTRON_RUN_AS_NODE", "VSCODE_CRASH_REPORTER_PROCESS_TYPE",
  "VSCODE_ESM_ENTRYPOINT", "VSCODE_HANDLES_UNCAUGHT_ERRORS", "VSCODE_IPC_HOOK",
] as const;

export function cleanVsCodeGuiEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const clean = { ...env };
  for (const key of GUI_MODE_KEYS) delete clean[key];
  return clean;
}

/** Opt-in input conversion for the VS Code test launcher, never the product or ACP. */
export function windowTestProxy(env: NodeJS.ProcessEnv): string | undefined {
  if (env.TOMCAT_E2E_LLM_PROXY !== undefined) return env.TOMCAT_E2E_LLM_PROXY;
  const name = env.TOMCAT_E2E_WINDOW_PROXY_ENV;
  if (name === undefined) return undefined;
  if (!/^(HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|http_proxy|https_proxy|all_proxy)$/.test(name)) {
    throw new Error("TOMCAT_E2E_WINDOW_PROXY_ENV must select one HTTP_PROXY, HTTPS_PROXY or ALL_PROXY alias");
  }
  return env[name]; // No fallback or NO_PROXY emulation; an absent value keeps the default fixture.
}

/** Use the extension's declared minimum, as the packaged-extension runner does. */
export async function minimumVsCodeVersion(): Promise<string> {
  const manifest = JSON.parse(await fs.readFile(path.resolve(__dirname, "../package.json"), "utf8"));
  const version = manifest.engines.vscode.replace(/^\^/, "");
  if (!/^[0-9]+\.[0-9]+\.[0-9]+$/.test(version)) throw new Error(`Cannot resolve minimum VS Code version: ${version}`);
  return version;
}

let guiLaunchActive = false;

export async function withVsCodeGuiEnvironment<T>(
  env: NodeJS.ProcessEnv,
  launch: (clean: NodeJS.ProcessEnv) => T | Promise<T>,
): Promise<T> {
  // test-electron merges process.env again. Both sources must be clean, and
  // overlapping scopes must not restore one another's temporary values.
  if (guiLaunchActive) throw new Error("overlapping VS Code GUI environment scopes are not supported");
  guiLaunchActive = true;
  const previous = GUI_MODE_KEYS.map((key) => [key, process.env[key]] as const);
  try {
    for (const key of GUI_MODE_KEYS) delete process.env[key];
    return await launch(cleanVsCodeGuiEnvironment(env));
  } finally {
    for (const [key, value] of previous) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    guiLaunchActive = false;
  }
}

async function reserveCapturePort(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const port = (server.address() as net.AddressInfo).port;
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

type GuiOptions = {
  extensionTestsEnv?: NodeJS.ProcessEnv;
  launchArgs?: string[];
  vscodeExecutablePath?: string;
  stdout?: Writable;
  stderr?: Writable;
};

/** No runtime test-electron import: install-e2e controls its import-time cwd. */
export async function runVsCodeGuiTests<T extends GuiOptions>(
  run: (options: T) => Promise<number>,
  options: T,
): Promise<number> {
  const env = options.extensionTestsEnv ?? process.env;
  const launchArgs = [...(options.launchArgs ?? [])];
  if (!launchArgs.some(arg => arg === "--locale" || arg.startsWith("--locale="))) launchArgs.push("--locale=en");
  if (!launchArgs.includes("--password-store=basic")) launchArgs.push("--password-store=basic");
  // These disposable acceptance profiles must not access the OS credential
  // store. password-store=basic alone does not prevent macOS keychain dialogs.
  if (!launchArgs.includes("--use-inmemory-secretstorage")) launchArgs.push("--use-inmemory-secretstorage");
  const existingPort = launchArgs.find((arg) => arg.startsWith("--remote-debugging-port="));
  const cdpPort = existingPort ? Number(existingPort.split("=")[1]) : await reserveCapturePort();
  if (!Number.isInteger(cdpPort) || cdpPort <= 0 || cdpPort > 65535) throw new Error("Invalid VS Code capture port");
  if (!existingPort) launchArgs.push(`--remote-debugging-port=${cdpPort}`);
  const root = env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR ?? path.join(os.tmpdir(), "tomcat-vscode-runs");
  await fs.mkdir(root, { recursive: true });
  const artifacts = await fs.mkdtemp(path.join(root, "gui-"));
  const secrets = Object.entries({ ...process.env, ...env })
    .filter(([key, value]) => /(?:key|token|password|secret)/i.test(key) && value && value.length >= 8)
    .map(([, value]) => value!);
  const redact = (text: string) => secrets.reduce((result, secret) => result.split(secret).join("[REDACTED]"), text);
  const tails = { stdout: "", stderr: "" };
  const tee = (kind: "stdout" | "stderr") => new Writable({
    write(chunk, _encoding, done) {
      tails[kind] = (tails[kind] + String(chunk)).slice(-65_536);
      (options[kind] ?? process[kind]).write(chunk);
      done();
    },
  });
  const userDataArg = options.launchArgs?.find((arg) => arg.startsWith("--user-data-dir="));
  const userData = userDataArg?.slice("--user-data-dir=".length);
  let outcome: unknown = { status: "not-started" };
  let failed = false;
  console.log(`VS Code GUI artifacts: ${artifacts}`);
  try {
    await fs.writeFile(path.join(artifacts, "launch.json"), redact(JSON.stringify({
      executable: options.vscodeExecutablePath,
      args: launchArgs,
      nodeVersion: process.version,
      modeKeys: GUI_MODE_KEYS.map((key) => ({ key, parent: process.env[key] !== undefined, supplied: env[key] !== undefined })),
    }, null, 2)));
    const code = await withVsCodeGuiEnvironment(env, (clean) => run({
      ...options,
      launchArgs,
      extensionTestsEnv: { ...clean, TOMCAT_E2E_CDP_PORT: String(cdpPort), TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifacts },
      stdout: tee("stdout"), stderr: tee("stderr"),
    }));
    outcome = { status: code === 0 ? "passed" : "failed", exitCode: code };
    if (code !== 0) throw new Error(`VS Code test runner exited with ${code}`);
    return code;
  } catch (error) {
    failed = true;
    outcome = { status: "failed", error: String(error) };
    throw error;
  } finally {
    try {
      await fs.writeFile(path.join(artifacts, "result.json"), redact(JSON.stringify(outcome, null, 2)));
      for (const kind of ["stdout", "stderr"] as const) {
        await fs.writeFile(path.join(artifacts, `${kind}.log`), redact(tails[kind]));
      }
      if (userData) await copyLogs(path.join(userData, "logs"), path.join(artifacts, "host-logs"), redact);
      const entries = await fs.readdir(artifacts);
      const screenshotDir = env.TOMCAT_ACCEPT_SCREENSHOTS_DIR;
      const external = screenshotDir ? await fs.readdir(screenshotDir).catch((error: NodeJS.ErrnoException) => {
        if (error.code === "ENOENT") return [];
        throw error;
      }) : [];
      if (external.some((entry) => entry.endsWith(".png"))) {
        await fs.writeFile(path.join(artifacts, "screenshots.json"), redact(JSON.stringify({ directory: screenshotDir, files: external }, null, 2)));
      }
      if (![...entries, ...external].some((entry) => entry.endsWith(".png"))) {
        await fs.writeFile(path.join(artifacts, "screenshots-not-produced.txt"), "No PNG was produced in this run; visual acceptance is not established.\n");
      }
    } catch (error) {
      if (!failed) throw error;
      console.warn(`Secondary artifact collection failure (original launch error retained): ${String(error)}`);
    }
  }
}

async function copyLogs(source: string, target: string, redact: (text: string) => string): Promise<void> {
  const entries = await fs.readdir(source, { withFileTypes: true }).catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return [];
    throw error;
  });
  if (!entries.length) return;
  await fs.mkdir(target, { recursive: true });
  for (const entry of entries) {
    const from = path.join(source, entry.name);
    const to = path.join(target, entry.name);
    if (entry.isDirectory()) await copyLogs(from, to, redact);
    else if (entry.isFile()) await fs.writeFile(to, redact(await fs.readFile(from, "utf8")));
    // Do not follow symlinks outside this launch's owned user-data directory.
  }
}
