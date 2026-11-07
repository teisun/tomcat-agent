// Runs the fixed, unchanged Multica CLI and its real LoadConfig path probe.
import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as os from "node:os";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { windowsLoginPath, readWindowsUserPath } from "./windowsUserPath";

export const MULTICA_REVISION = "8ea00434a47f60b92aaf1504ecb29a2b70f82d0c";
const exec = promisify(execFile);
const hash = async (file: string) => createHash("sha256").update(await fs.readFile(file)).digest("hex");

export async function prepareMulticaDiscovery(source: string, output: string) {
  const revision = (await exec("git", ["-C", source, "rev-parse", `${MULTICA_REVISION}^{commit}`])).stdout.trim();
  assert.equal(revision, MULTICA_REVISION);
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-multica-build-"));
  try {
    const archive = path.join(root, "source.tar");
    await exec("git", ["-C", source, "archive", "--format=tar", `--output=${archive}`, revision, "server"]);
    const sourceArchiveSha256 = await hash(archive);
    await exec("tar", ["-xf", archive, "-C", root]);
    const moduleRoot = path.join(root, "server");
    const probeRoot = path.join(moduleRoot, "cmd/tomcat-discovery-acceptance");
    await fs.mkdir(probeRoot);
    await fs.copyFile(path.join(__dirname, "fixtures/multica-cli-discovery/main.go"), path.join(probeRoot, "main.go"));
    await fs.mkdir(output, { recursive: true });
    const suffix = process.platform === "win32" ? ".exe" : "";
    const binary = path.join(output, `multica${suffix}`);
    const configProbe = path.join(output, `multica-config-probe${suffix}`);
    const env = { ...process.env, GOWORK: "off", GOOS: process.platform === "win32" ? "windows" : process.platform, GOARCH: process.arch === "x64" ? "amd64" : process.arch };
    for (const [target, file] of [["./cmd/multica", binary], ["./cmd/tomcat-discovery-acceptance", configProbe]]) {
      await exec("go", ["build", "-mod=readonly", "-buildvcs=false", "-o", file, target], { cwd: moduleRoot, env, timeout: 600_000, maxBuffer: 4 * 1024 * 1024 });
    }
    const evidence = { revision, sourceArchiveSha256, binary, configProbe, multicaSha256: await hash(binary), configProbeSha256: await hash(configProbe), goVersion: (await exec("go", ["version"])).stdout.trim() };
    await fs.writeFile(path.join(output, "multica-build.json"), JSON.stringify(evidence, null, 2));
    return evidence;
  } finally { await fs.rm(root, { recursive: true, force: true, maxRetries: 3 }); }
}

if (path.basename(process.argv[1] ?? "") === "multicaCliDiscovery.ts") {
  const [source, output] = process.argv.slice(2);
  if (!source || !output) throw new Error("Usage: multicaCliDiscovery.ts <fixed Multica git checkout> <owned output directory>");
  prepareMulticaDiscovery(path.resolve(source), path.resolve(output)).then(result => console.log(JSON.stringify(result)), error => { console.error(error); process.exitCode = 1; });
}

export async function verifyMulticaDiscovery(fixture: { env: NodeJS.ProcessEnv; homePath: string; workspacePath: string }, expectedHash: string, artifacts: string) {
  const binary = process.env.TOMCAT_MULTICA_ACCEPT_BINARY;
  const configProbe = process.env.TOMCAT_MULTICA_CONFIG_PROBE;
  assert.ok(binary && configProbe, "build the pinned Multica CLI and Tomcat-side config probe before native discovery acceptance");
  assert.equal(process.env.TOMCAT_MULTICA_ACCEPT_REVISION, MULTICA_REVISION);
  const env = { ...fixture.env };
  for (const key of Object.keys(env)) if (key.startsWith("MULTICA_")) delete env[key];
  // No login, daemon startup, model request, launcher, or explicit Tomcat path.
  Object.assign(env, { MULTICA_DAEMON_ID: "00000000-0000-4000-8000-000000000001", MULTICA_DAEMON_AUTO_UPDATE: "false" });
  const expected = path.join(fixture.homePath, ".local/bin", process.platform === "win32" ? "tomcat.exe" : "tomcat");
  if (process.platform === "win32") {
    const loginPath = await windowsLoginPath(); // product-written registry, not an injected expected directory
    for (const name of Object.keys(env)) if (name.toLowerCase() === "path") delete env[name];
    env.PATH = loginPath;
  } else {
    env.PATH = "/usr/bin:/bin"; // GUI-style environment: real login shell must rescue discovery
    assert.ok(env.SHELL);
  }
  const options = { cwd: fixture.workspacePath, env, timeout: 30_000, maxBuffer: 1024 * 1024 };
  const result = await exec(binary, ["daemon", "probe-runtimes", "--profile", "cli-discovery"], options);
  const probe = JSON.parse(result.stdout);
  assert.equal(probe.probe_result, "success");
  assert.equal(probe.provider_summary.tomcat, 1);
  const selected = JSON.parse((await exec(configProbe, [], options)).stdout) as { path: string; command: string };
  assert.equal(selected.command, "tomcat", "no MULTICA_TOMCAT_PATH override");
  assert.equal(await fs.realpath(selected.path), await fs.realpath(expected), "the actual Multica LoadConfig entry must match the VSIX user entry");
  assert.equal(await hash(selected.path), expectedHash);
  const version = (await exec(selected.path, ["--version"], options)).stdout.trim();
  const shellPath = process.platform === "win32"
    ? (await exec("where.exe", ["tomcat"], options)).stdout.trim().split(/\r?\n/)[0]
    : (await exec(env.SHELL!, ["-lc", "command -v tomcat"], options)).stdout.trim();
  assert.equal(await fs.realpath(shellPath), await fs.realpath(expected), "fresh terminal discovery must agree too");
  const evidence = { status: "passed", sourceRevision: MULTICA_REVISION, multicaSha256: await hash(binary), configProbeSha256: await hash(configProbe),
    probe, selected, version, cliSha256: expectedHash, terminalPath: shellPath, dedicatedPathOverride: false,
    ...(process.platform === "win32" ? { userPath: await readWindowsUserPath() } : { initialPath: env.PATH, shell: env.SHELL }) };
  await fs.writeFile(path.join(artifacts, "multica-discovery.json"), JSON.stringify(evidence, null, 2));
  return evidence;
}
