import * as path from "node:path";
import * as os from "node:os";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type Socket } from "node:net";
import { createInterface } from "node:readline";
import { afterEach, expect, it, vi } from "vitest";
import { TomcatMessenger } from "../TomcatMessenger";
import { connectSharedService, ServiceVersionMismatchError, type SharedConnectionOptions, type SharedServiceInfo } from "../sharedConnection";

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse()) await cleanup(); });

async function fixture(options: { stopped?: boolean; version?: string; protocol?: number; changedIdentity?: boolean; holdHello?: boolean } = {}) {
  const root = await mkdtemp(path.join(process.platform === "darwin" ? "/tmp" : os.tmpdir(), "tc-connect-"));
  const endpoint = process.platform === "win32" ? String.raw`\\.\pipe\tc-connect-${randomUUID()}` : path.join(root, "serve.sock");
  const info: SharedServiceInfo = { service: "tomcat", running: true, protocolVersion: options.protocol ?? 1,
    serverVersion: options.version ?? "0.1.80", instanceId: "instance", pid: process.pid, agentId: "main", endpoint };
  const received: Array<Record<string, unknown>> = [];
  const sockets = new Set<Socket>();
  let sawHello!: () => void;
  const helloReceived = new Promise<void>(resolve => { sawHello = resolve; });
  const server = createServer(socket => {
    sockets.add(socket);
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
      sawHello();
      if (!options.holdHello) socket.write(JSON.stringify({ ...info, instanceId: options.changedIdentity ? "replacement" : info.instanceId }) + "\n");
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
  await writeFile(script, `const fs = require("node:fs");
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify(args) + "\\n");
console.log(args[0] === "--version" ? "tomcat 0.1.80" : ${JSON.stringify(JSON.stringify(options.stopped ? { service: "tomcat", running: false } : info))});
`);
  const abort = new AbortController();
  const connectOptions: SharedConnectionOptions = {
    executable: process.execPath, extraArgs: [script], cwd: root, signal: abort.signal,
    allowStart: true, env: { TEST_CONTROLLER_VALUE: "private-value" }, onStderr: vi.fn(),
  };
  return { abort, connectOptions, received, helloReceived,
    calls: async () => (await readFile(log, "utf8")).trim().split("\n").map(line => JSON.parse(line)),
  };
}

it("uses CLI discovery then native IPC with the controller environment, never logging the envelope", async () => {
  const f = await fixture();
  const connection = await connectSharedService(f.connectOptions);
  expect(connection.info.instanceId).toBe("instance");
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
  expect(f.received[0].protocol).toBe("native");
  expect((f.received[0].environment as Record<string, string>).TEST_CONTROLLER_VALUE).toBe("private-value");
  expect(f.connectOptions.onStderr).not.toHaveBeenCalled();
  connection.socket.destroy();
});

it("hands the paused handshake socket to the normal NDJSON reader", async () => {
  const f = await fixture();
  const messenger = new TomcatMessenger(f.connectOptions);
  try {
    await expect(messenger.request({ type: "get_state" }, 2_000)).resolves.toMatchObject({ success: true, payload: { native: true } });
    expect(f.received.map(frame => frame.type ?? frame.protocol)).toEqual(["native", "get_state"]);
  } finally { await messenger.disposeAsync(); }
});

it("automatic recovery probes status without starting a stopped service", async () => {
  const f = await fixture({ stopped: true });
  await expect(connectSharedService({ ...f.connectOptions, allowStart: false })).rejects.toThrow("unavailable");
  expect(await f.calls()).toEqual([["--version"], ["serve", "status"]]);
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

it("rejects an instance change between discovery and socket handshake", async () => {
  const f = await fixture({ changedIdentity: true });
  await expect(connectSharedService(f.connectOptions)).rejects.toThrow("identity changed");
  expect(f.received).toHaveLength(1);
});

it("cancels a pending local handshake without leaking or restarting the service", async () => {
  const f = await fixture({ holdHello: true });
  const rejected = expect(connectSharedService(f.connectOptions)).rejects.toThrow("cancelled");
  await f.helloReceived;
  f.abort.abort();
  await rejected;
  expect(await f.calls()).toEqual([["--version"], ["serve", "start"]]);
});
