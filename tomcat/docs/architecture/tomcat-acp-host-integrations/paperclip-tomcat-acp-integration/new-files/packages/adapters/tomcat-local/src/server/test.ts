import { randomUUID } from "node:crypto";
import type { AdapterEnvironmentCheck, AdapterEnvironmentTestContext, AdapterEnvironmentTestResult } from "@paperclipai/adapter-utils";
import { asString, ensureAbsoluteDirectory, ensureCommandResolvable, ensurePathInEnv, parseObject, runChildProcess } from "@paperclipai/adapter-utils/server-utils";

export async function testEnvironment(ctx: AdapterEnvironmentTestContext): Promise<AdapterEnvironmentTestResult> {
  const checks: AdapterEnvironmentCheck[] = [];
  const result = (): AdapterEnvironmentTestResult => ({
    adapterType: "tomcat_local", testedAt: new Date().toISOString(), checks,
    status: checks.some(check => check.level === "error") ? "fail" : checks.some(check => check.level === "warn") ? "warn" : "pass",
  });
  const fail = (code: string, message: string, hint?: string) => {
    checks.push({ code, level: "error", message, hint });
    return result();
  };
  if (ctx.executionTarget?.kind === "remote") return fail("tomcat_local_target_required", "Tomcat local requires the host and service to share the same local workspace.");
  const [major = 0, minor = 0] = process.versions.node.split(".").map(Number);
  if (major < 24 || (major === 24 && minor < 11)) return fail("tomcat_node_unsupported", "Tomcat's Paperclip ACP runtime requires Node >=24.11.0.");
  const config = parseObject(ctx.config);
  if (asString(config.agentCommand, "").trim()) {
    checks.push({ code: "tomcat_custom_command", level: "warn", message: "A custom ACP launcher is configured. Its availability and service compatibility will be checked when a run starts." });
    return result();
  }
  const cwd = asString(config.cwd, process.cwd());
  try { await ensureAbsoluteDirectory(cwd, { createIfMissing: false }); }
  catch { return fail("tomcat_cwd_invalid", "The configured working directory must be an existing absolute directory."); }
  const command = asString(config.command, "tomcat").trim();
  const env = Object.fromEntries(Object.entries(parseObject(config.env)).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
  try { await ensureCommandResolvable(command, cwd, ensurePathInEnv({ ...process.env, ...env })); }
  catch { return fail("tomcat_command_missing", "The Tomcat executable is unavailable.", "Install Tomcat with ACP support, or set its executable path."); }
  try { await ensureCommandResolvable("sh", cwd, ensurePathInEnv({ ...process.env, ...env })); }
  catch { return fail("tomcat_shell_missing", "The Tomcat ACP launcher requires a POSIX shell.", "Use macOS, Linux or WSL2 with sh available."); }
  const runId = `tomcat-env-${randomUUID()}`;
  const probe = (args: string[]) => runChildProcess(runId, command, args, { cwd, env, timeoutSec: 15, graceSec: 2, onLog: async () => {} });
  try {
    const version = await probe(["--version"]);
    const match = version.stdout.match(/^tomcat\s+((\d+)\.(\d+)\.(\d+)\S*)/m);
    if (version.timedOut || version.exitCode !== 0 || !match) return fail("tomcat_version_unavailable", "Could not read the Tomcat CLI version.");
    const help = await probe(["acp", "--help"]);
    if (help.timedOut || help.exitCode !== 0 || !help.stdout.includes("--skills-dir")) return fail("tomcat_acp_unavailable", "This Tomcat build does not expose the required ACP skills option.", "Use the shared-service ACP build of Tomcat.");
    // Read-only management probe: do not start/restart a service or create a conversation.
    const status = await probe(["serve", "status"]);
    if (status.timedOut || status.exitCode !== 0) return fail("tomcat_service_unreachable", "The shared Tomcat service status could not be read.", "Check the local Tomcat service configuration; this probe never restarts running tasks.");
    let service: Record<string, unknown>;
    try { service = parseObject(JSON.parse(status.stdout)); }
    catch { return fail("tomcat_service_invalid_response", "Tomcat returned an invalid service status."); }
    if (service.running === false) {
      checks.push({ code: "tomcat_service_stopped", level: "info", message: "Tomcat is installed; its shared service is stopped. Starting a task will start the service." });
    } else if (service.running === true && service.service === "tomcat") {
      checks.push({ code: "tomcat_service_ready", level: "info", message: `Tomcat's shared service is reachable (service version ${asString(service.serverVersion, "unknown")}).` });
    } else return fail("tomcat_service_invalid_response", "Tomcat returned an invalid service status.");
    checks.push({ code: "tomcat_probe_scope", level: "info", message: "Checked the executable and service only. No model request or conversation was created; model credentials are validated when a task runs." });
    return result();
  } catch {
    return fail("tomcat_probe_failed", "The Tomcat environment probe could not complete.", "Check the executable and local service configuration.");
  }
}
