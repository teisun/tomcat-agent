import * as path from "node:path";
import * as os from "node:os";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type Socket } from "node:net";
import { createInterface } from "node:readline";
import { afterEach, expect, it, vi } from "vitest";
import { TomcatMessenger } from "../TomcatMessenger";
import { connectSharedService, runSharedServiceAction, ServiceVersionMismatchError, type SharedConnectionOptions, type SharedServiceInfo } from "../sharedConnection";

const { cliInvocations } = vi.hoisted(() => ({ cliInvocations: vi.fn() }));
vi.mock("node:child_process", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:child_process")>();
  const { promisify } = await import("node:util");
  const run = promisify(actual.execFile);
  const execFile = (...args: Parameters<typeof actual.execFile>) => actual.execFile(...args);
  // runCli uses promisify.custom; record that call without replacing the real child process.
  Object.defineProperty(execFile, promisify.custom, {
    value: (...args: Parameters<typeof run>) => { cliInvocations(...args); return run(...args); },
  });
  return { ...actual, execFile };
});

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse()) await cleanup(); });

async function fixture(options: {
  stopped?: boolean; version?: string; protocol?: number; changedIdentity?: boolean; absentEndpoint?: boolean;
  holdHello?: boolean; malformedHello?: boolean; handshakeError?: string;
  holdReady?: boolean; readyError?: string; oldBuild?: boolean; actionExitCode?: number;
} = {}) {
  const root = await mkdtemp(path.join(process.platform === "darwin" ? "/tmp" : os.tmpdir(), "tc-connect-"));
  const endpoint = process.platform === "win32" ? String.raw`\\.\pipe\tc-connect-${randomUUID()}` : path.join(root, "serve.sock");
  const info: SharedServiceInfo = { service: "tomcat", running: true, protocolVersion: options.protocol ?? 1,
    serverVersion: options.version ?? "0.1.80", instanceId: "instance", pid: process.pid, agentId: "main",
    endpoint: options.absentEndpoint ? `${endpoint}-gone` : endpoint };
  const received: Array<Record<string, unknown>> = [];
  const bytes: Buffer[] = [];
  let sawEnvironment!: () => void;
  const environmentReceived = new Promise<void>(resolve => { sawEnvironment = resolve; });
  const sockets = new Set<Socket>();
  let sawHello!: () => void;
  const helloReceived = new Promise<void>(resolve => { sawHello = resolve; });
  const server = createServer(socket => {
    sockets.add(socket);
    socket.on("data", chunk => bytes.push(Buffer.from(chunk)));
    socket.on("error", () => undefined);
    socket.once("close", () => sockets.delete(socket));
    const lines = createInterface({ input: socket });
    lines.on("line", line => {
      const frame = JSON.parse(line) as Record<string, unknown>;
      received.push(frame);
      if (frame.type) {
        socket.write(JSON.stringify({ type: "response", id: frame.id, success: true, payload: { native: true } }) + "\n");
        return;
      }
      if (frame.protocol) {
        sawHello();
        if (!options.holdHello) socket.write(options.malformedHello ? "not-json\n" : JSON.stringify({ ...info,
          instanceId: options.changedIdentity ? "replacement" : info.instanceId,
          ...(options.handshakeError ? { error: options.handshakeError } : {}) }) + "\n");
      } else if ("environment" in frame) {
        sawEnvironment();
        if (!options.holdReady) socket.write(JSON.stringify(options.oldBuild
          ? { type: "response", success: false, error: "missing field type" }
          : options.readyError ? { error: options.readyError } : { ready: true }) + "\n");
      }
    });
  });
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(endpoint, resolve); });
  cleanups.push(async () => {
    await new Promise<void>((resolve, reject) => {
      server.close(error => error ? reject(error) : resolve());
      for (const socket of sockets) socket.destroy();
    });
    await rm(root, { recursive: true, force: true });
  });
  const script = path.join(root, "cli.cjs");
  const log = path.join(root, "commands.jsonl");
  const envLog = path.join(root, "case-environment.json");
  await writeFile(script, `const fs = require("node:fs");
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify(args) + "\\n");
if (args[0] === "serve" && ["stop", "restart"].includes(args[1]) && ${options.actionExitCode ?? 0}) {
  console.error("management failed"); process.exit(${options.actionExitCode ?? 0});
}
fs.writeFileSync(${JSON.stringify(envLog)}, JSON.stringify(Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() === "TEST_ENV_CASE"))));
console.log(args[0] === "--version" ? "tomcat 0.1.80" : ${JSON.stringify(JSON.stringify(options.stopped ? { service: "tomcat", running: false } : info))});
`);
  const abort = new AbortController();
  const connectOptions: SharedConnectionOptions = {
    executable: process.execPath, extraArgs: [script], cwd: root, signal: abort.signal,
    allowStart: true, env: { TEST_CONTROLLER_VALUE: "private-value" }, onStderr: vi.fn(),
  };
  return { abort, connectOptions, received, helloReceived, environmentReceived,
    receivedBytes: () => Buffer.concat(bytes).toString("utf8"),
    releaseReady: () => { for (const socket of sockets) socket.write('{"ready":true}\n'); },
    childEnvironment: async () => JSON.parse(await readFile(envLog, "utf8")),
    calls: async () => (await readFile(log, "utf8")).trim().split("\n").map(line => JSON.parse(line)),
  };
}

it.each([ ["stop", 80_000], ["restart", 150_000] ] as const)("gives %s its complete CLI budget", async (action, timeout) => {
  const f = await fixture();
  await runSharedServiceAction(f.connectOptions, action);
  expect(await f.calls()).toEqual([["serve", action]]);
  expect(cliInvocations).toHaveBeenCalledExactlyOnceWith(process.execPath, [...f.connectOptions.extraArgs!, "serve", action],
    expect.objectContaining({ timeout, signal: f.abort.signal, windowsHide: true }));
});

it("preserves management failure and stderr instead of reporting success", async () => {
  const f = await fixture({ actionExitCode: 7 });
  await expect(runSharedServiceAction(f.connectOptions, "restart")).rejects.toMatchObject({ code: 7 });
  expect(f.connectOptions.onStderr).toHaveBeenCalledWith(Buffer.from("management failed\n"));
  expect(await f.calls()).toEqual([["serve", "restart"]]);
});

it("rejects a vanished discovered endpoint without an uncaught readline error or environment transmission", async () => {
  const f = await fixture({ absentEndpoint: true });
  await expect(connectSharedService(f.connectOptions)).rejects.toMatchObject({ code: "ENOENT" });
  expect(f.received).toHaveLength(0);
  expect(f.receivedBytes()).toBe("");
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
  expect(f.connectOptions.onStderr).not.toHaveBeenCalled();
});

it("uses CLI discovery then native IPC with the controller environment, never logging the envelope", async () => {
  const f = await fixture();
  const connection = await connectSharedService(f.connectOptions);
  expect(connection.info.instanceId).toBe("instance");
  for (const args of [["--version"], ["serve", "start"]]) {
    expect(cliInvocations).toHaveBeenCalledWith(process.execPath, [...f.connectOptions.extraArgs!, ...args], expect.objectContaining({ timeout: 35_000 }));
  }
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
  expect(f.received[0].protocol).toBe("native");
  expect("environment" in f.received[0]).toBe(false);
  expect(f.received.length).toBe(2);
  expect((f.received[1].environment as Record<string, string>).TEST_CONTROLLER_VALUE).toBe("private-value");
  expect(f.connectOptions.onStderr).not.toHaveBeenCalled();
  connection.socket.destroy();
});

it.each(["upper", undefined])("encodes case aliases exactly as the actual CLI child receives them (%s)", async value => {
  const f = await fixture();
  f.connectOptions.env = { ...f.connectOptions.env, TEST_ENV_CASE: value, test_env_case: "lower" };
  const connection = await connectSharedService(f.connectOptions);
  try {
    const sent = f.received[1].environment as Record<string, string>;
    const aliases = Object.fromEntries(Object.entries(sent).filter(([key]) => key.toUpperCase() === "TEST_ENV_CASE"));
    expect(aliases).toEqual(await f.childEnvironment());
    expect(Object.keys(aliases)).toHaveLength(process.platform === "win32" ? (value === undefined ? 0 : 1) : (value === undefined ? 1 : 2));
  } finally { connection.socket.destroy(); }
});

it("hands the paused handshake socket to the normal NDJSON reader", async () => {
  const f = await fixture();
  const messenger = new TomcatMessenger(f.connectOptions);
  try {
    await expect(messenger.request({ type: "get_state" }, 2_000)).resolves.toMatchObject({ success: true, payload: { native: true } });
    expect(f.received.map(frame => frame.type ?? frame.protocol ?? "environment")).toEqual(["native", "environment", "get_state"]);
  } finally { await messenger.disposeAsync(); }
});

it("automatic recovery probes status without starting a stopped service", async () => {
  const f = await fixture({ stopped: true });
  await expect(connectSharedService({ ...f.connectOptions, allowStart: false })).rejects.toThrow("unavailable");
  expect(await f.calls()).toEqual([["--version"], ["serve", "status"]]);
  expect(cliInvocations).toHaveBeenLastCalledWith(process.execPath, [...f.connectOptions.extraArgs!, "serve", "status"], expect.objectContaining({ timeout: 35_000 }));
  expect(f.received).toHaveLength(0);
});

it.each([{ version: "0.0.1", error: "service_version_mismatch" }, { protocol: 99, error: "incompatible protocol" }])(
  "rejects incompatible metadata without a private-process fallback ($error)", async ({ error, ...options }) => {
    const f = await fixture(options);
    const connection = connectSharedService(f.connectOptions);
    await expect(connection).rejects.toThrow(error);
    if (options.version) {
      await expect(connection).rejects.toBeInstanceOf(ServiceVersionMismatchError);
      await expect(connection).rejects.toMatchObject({ running: options.version, client: "0.1.80" });
    }
    expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
    expect(f.received).toHaveLength(0);
  },
);

it("rejects a service denial even when its identity and version match", async () => {
  const f = await fixture({ handshakeError: "invalid_connection_environment" });
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow("invalid_connection_environment");
  expect(f.received).toHaveLength(1);
  expect(f.received[0].protocol).toBe("native");
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
});

it("rejects an instance change between discovery and socket handshake", async () => {
  const f = await fixture({ changedIdentity: true });
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow("identity changed");
  expect(f.received).toHaveLength(1);
  expect(f.receivedBytes().includes("environment")).toBe(false);
  expect(f.receivedBytes().includes("private-value")).toBe(false);
});

it.each([{ malformedHello: true }, { holdHello: true }])("never sends any environment bytes before a valid identity ($malformedHello)", async options => {
  const f = await fixture(options);
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow();
  expect(f.receivedBytes().includes("environment")).toBe(false);
  expect(f.receivedBytes().includes("private-value")).toBe(false);
  expect(f.connectOptions.onStderr).not.toHaveBeenCalled();
}, 10_000);

it("waits for environment acknowledgement before returning the socket", async () => {
  const f = await fixture({ holdReady: true });
  let connected = false;
  const pending = connectSharedService(f.connectOptions).then(value => { connected = true; return value; });
  await f.environmentReceived;
  expect(connected).toBe(false);
  f.releaseReady();
  (await pending).socket.destroy();
});

it("preserves an environment rejection instead of admitting a messenger", async () => {
  const f = await fixture({ readyError: "invalid_connection_environment" });
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow("invalid_connection_environment");
  expect(f.received.length).toBe(2);
  expect(f.connectOptions.onStderr).not.toHaveBeenCalled();
});

it("classifies a same-version old native reply as a service build mismatch", async () => {
  const f = await fixture({ oldBuild: true });
  await expect(connectSharedService(f.connectOptions)).rejects.toMatchObject({
    name: "ServiceVersionMismatchError", running: "0.1.80", client: "0.1.80",
  });
  expect(f.received.length).toBe(2);
  expect("environment" in f.received[0]).toBe(false);
});

it("applies the original handshake deadline while awaiting ready", async () => {
  const f = await fixture({ holdReady: true });
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow("handshake timed out");
  expect(f.received.length).toBe(2);
}, 10_000);

it("cancels while awaiting ready without replaying the environment", async () => {
  const f = await fixture({ holdReady: true });
  const pending = expect(connectSharedService(f.connectOptions)).rejects.toThrow("cancelled");
  await f.environmentReceived;
  f.abort.abort();
  await pending;
  expect(f.received.length).toBe(2);
});

it("cancels a pending local handshake without leaking or restarting the service", async () => {
  const f = await fixture({ holdHello: true });
  const rejected = expect(connectSharedService(f.connectOptions)).rejects.toThrow("cancelled");
  await f.helloReceived;
  f.abort.abort();
  await rejected;
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
});
