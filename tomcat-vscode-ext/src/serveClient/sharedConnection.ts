import { execFile } from "node:child_process";
import { connect, type Socket } from "node:net";
import { createInterface } from "node:readline";
import { promisify } from "node:util";
import type { Duplex } from "node:stream";

const execFileAsync = promisify(execFile);
const LOCAL_PROTOCOL_VERSION = 1;
// service.rs: connect + hello (10s) + STOP_TIMEOUT (60s), plus 10s scheduling margin.
const STOP_COMMAND_TIMEOUT_MS = 80_000;
// stop (70s) + start probes/lock/readiness (up to 70s), plus 10s margin.
const RESTART_COMMAND_TIMEOUT_MS = 150_000;

export interface SharedServiceInfo {
  service: "tomcat";
  protocolVersion: number;
  serverVersion: string;
  instanceId: string;
  pid: number;
  agentId: string;
  endpoint: string;
  running: true;
}

export class ServiceVersionMismatchError extends Error {
  constructor(readonly running: string, readonly client: string) {
    super(`service_version_mismatch: running ${running}, client ${client}; restart the shared service to upgrade (all tasks will stop)`);
    this.name = "ServiceVersionMismatchError";
  }
}

export interface SharedConnection {
  socket: Duplex;
  info: SharedServiceInfo;
}

export interface SharedConnectionOptions {
  executable: string;
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  extraArgs?: string[];
  signal: AbortSignal;
  allowStart: boolean;
  onStderr(chunk: Buffer): void;
}

export type SharedConnectionFactory = (options: SharedConnectionOptions) => Promise<SharedConnection>;

async function runCli(options: Omit<SharedConnectionOptions, "allowStart">, args: string[], timeout = 35_000): Promise<string> {
  try {
    const result = await execFileAsync(options.executable, [...(options.extraArgs ?? []), ...args], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      windowsHide: true,
      signal: options.signal,
      timeout,
      maxBuffer: 1024 * 1024,
      encoding: "utf8",
    });
    if (result.stderr) options.onStderr(Buffer.from(result.stderr));
    return result.stdout;
  } catch (error) {
    const stderr = (error as { stderr?: unknown }).stderr;
    if (typeof stderr === "string" && stderr) options.onStderr(Buffer.from(stderr));
    throw error;
  }
}

function serviceInfo(value: unknown): SharedServiceInfo {
  const info = value as (Partial<SharedServiceInfo> & { error?: unknown }) | null;
  if (!info || info.running !== true || info.service !== "tomcat" ||
      info.protocolVersion !== LOCAL_PROTOCOL_VERSION || typeof info.serverVersion !== "string" ||
      typeof info.instanceId !== "string" || !info.instanceId || typeof info.pid !== "number" ||
      typeof info.agentId !== "string" || typeof info.endpoint !== "string" || !info.endpoint) {
    throw new Error("Tomcat shared service is unavailable or has an incompatible protocol. Reconnect explicitly to start it.");
  }
  if (typeof info.error === "string") {
    throw new Error(`Tomcat shared service rejected the connection: ${info.error}`);
  }
  return info as SharedServiceInfo;
}

export async function runSharedServiceAction(
  options: Omit<SharedConnectionOptions, "allowStart">,
  action: "stop" | "restart",
): Promise<void> {
  await runCli(options, ["serve", action], action === "restart" ? RESTART_COMMAND_TIMEOUT_MS : STOP_COMMAND_TIMEOUT_MS);
}

export const connectSharedService: SharedConnectionFactory = async (options) => {
  // Status is intentionally non-starting: another window stopping the service
  // must not cause this window's background reconnect to resurrect it.
  const version = (await runCli(options, ["--version"])).trim().match(/^tomcat\s+(\S+)$/u)?.[1];
  if (!version) throw new Error("Unable to determine the Tomcat CLI version.");
  const info = serviceInfo(JSON.parse(await runCli(options, ["serve", options.allowStart ? "start" : "status"])));
  if (info.serverVersion !== version) {
    throw new ServiceVersionMismatchError(info.serverVersion, version);
  }
  options.signal.throwIfAborted();
  const socket: Socket = connect({ path: info.endpoint });
  try {
    await new Promise<void>((resolve, reject) => {
      const lines = createInterface({ input: socket, crlfDelay: Infinity });
      const timer = setTimeout(() => finish(new Error("Tomcat local handshake timed out")), 5_000);
      const abort = () => finish(new Error("Tomcat connection cancelled"));
      const end = () => finish(new Error("Tomcat closed before its local handshake"));
      const finish = (error?: Error) => {
        clearTimeout(timer);
        options.signal.removeEventListener("abort", abort);
        // Keep the one-shot error listener until the socket's next error; the
        // messenger attaches its lifetime listener after this promise resolves.
        socket.removeListener("end", end);
        lines.close();
        if (error) reject(error); else resolve();
      };
      options.signal.addEventListener("abort", abort, { once: true });
      socket.once("error", finish);
      socket.once("end", end);
      lines.once("line", (line) => {
        try {
          const hello = serviceInfo(JSON.parse(line));
          if (hello.instanceId !== info.instanceId || hello.serverVersion !== version || hello.endpoint !== info.endpoint) {
            throw new Error("Tomcat service identity changed during connection; reconnect without replaying requests.");
          }
          finish();
        } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
      });
      socket.once("connect", () => {
        const environment = { ...process.env, ...options.env };
        if (process.platform === "win32") {
          // Match Node's spawn/execFile: first case-insensitive key in sorted order,
          // including undefined values that intentionally remove a variable.
          const seen = new Set<string>();
          for (const key of Object.keys(environment).sort()) {
            const folded = key.toUpperCase();
            if (seen.has(folded)) delete environment[key];
            else seen.add(folded);
          }
        }
        // Do not log this envelope: it contains the controller's environment.
        socket.write(`${JSON.stringify({
          service: "tomcat", protocolVersion: LOCAL_PROTOCOL_VERSION, clientVersion: version,
          protocol: "native", environment,
        })}\n`);
      });
      if (options.signal.aborted) abort();
    });
    options.signal.throwIfAborted();
    if (socket.destroyed || socket.readableEnded) throw new Error("Tomcat closed during connection");
    // No session is selected yet, so the server sends no events before native
    // initialize. The messenger installs its normal NDJSON reader next.
    return { socket, info };
  } catch (error) {
    socket.destroy();
    throw error;
  }
};
