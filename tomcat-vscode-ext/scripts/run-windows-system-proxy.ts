// Explicit GitHub Windows acceptance: real registry -> real Rust/VS Code -> local controlled proxy.
import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as http from "node:http";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { promisify } from "node:util";
import { downloadAndUnzipVSCode } from "@vscode/test-electron";
import { withWindowsSystemProxy } from "./windowsSystemProxy";
import { resolveCargoCommand } from "./resolveCargoCommand";
import { runCommand } from "./runCommand";

const extensionRoot = path.resolve(__dirname, "..");
const rustRoot = path.resolve(extensionRoot, "../tomcat");
const exec = promisify(execFile);

// Avoid adding libuv child Jobs around the service-bearing tests. `execFile`
// does not forward detached to spawn; detached here still inherits host policy.
export async function runServiceTest(file: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv; timeout: number }) {
  return await new Promise<{ stdout: string; stderr: string }>((resolve, reject) => {
    const child = spawn(file, args, { ...options, detached: process.platform === "win32", windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    let failure: Error | undefined;
    const append = (data: Buffer, stream: "stdout" | "stderr") => {
      if (failure) return;
      if (stream === "stdout") stdout += String(data); else stderr += String(data);
      if (stdout.length + stderr.length > 16 * 1024 * 1024) {
        failure = new Error("Acceptance command output exceeded 16 MiB");
        child.kill();
      }
    };
    child.stdout.on("data", data => append(data, "stdout"));
    child.stderr.on("data", data => append(data, "stderr"));
    child.once("error", error => { failure = error; });
    child.once("close", (code, signal) => {
      if (failure || code !== 0 || child.killed) {
        reject(Object.assign(failure ?? new Error(`Acceptance command failed: ${file} (${code}, ${signal})`), { stdout, stderr }));
      } else resolve({ stdout, stderr });
    });
  });
}

async function main() {
  assert.equal(process.platform, "win32", "Windows acceptance only");
  assert.equal(process.env.GITHUB_ACTIONS, "true", "never mutate personal-machine proxy settings");
  const artifacts = path.resolve(process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR ?? path.join(extensionRoot, "../.agents/shots/windows-system-proxy"));
  await fs.mkdir(artifacts, { recursive: true });
  const env = { ...process.env };
  for (const name of Object.keys(env)) if (/^(http|https|all|no)_proxy$/i.test(name)) delete env[name];
  const built = await exec(resolveCargoCommand(), ["test", "--locked", "--features", "test-streamable-http-server", "--test", "serve_shared_e2e", "--no-run", "--message-format=json"],
    { cwd: rustRoot, env, timeout: 3_600_000, maxBuffer: 32 * 1024 * 1024 });
  const outputs = built.stdout.split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
  const artifact = (name: string) => {
    const entry = outputs.find(item => item.reason === "compiler-artifact" && item.target.name === name && item.executable);
    assert.ok(entry, `missing current-build executable: ${name}`);
    return entry.executable as string;
  };
  const testExe = artifact("serve_shared_e2e");
  const mcpExe = artifact("test_streamable_http_server");
  // Download/build before changing system proxy; subsequent launches use this exact Code.exe.
  env.VSCODE_EXECUTABLE_PATH = process.env.VSCODE_EXECUTABLE_PATH ?? await downloadAndUnzipVSCode({ version: "1.101.0" });
  runCommand("npm", ["run", "build"], { cwd: extensionRoot });
  const bound = path.join(artifacts, "mcp-bound-address.txt");
  // A fresh path prevents accepting a stale port from an earlier run.
  const owned = await fs.mkdtemp(path.join(artifacts, "mcp-"));
  const addressFile = path.join(owned, "address");
  const mcp = spawn(mcpExe, ["--port", "0"], { env: { ...env, MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE: addressFile }, stdio: ["ignore", "ignore", "pipe"], windowsHide: true });
  let mcpLog = "";
  mcp.stderr.on("data", data => { mcpLog += String(data); });
  let mcpError: Error | undefined;
  mcp.on("error", error => { mcpError = error; });
  const requests: { method: string; url: string; rpcMethod?: string; body?: string }[] = [];
  const connections = new Set<import("node:net").Socket>();
  const proxy = http.createServer((request, response) => {
    void (async () => {
      const url = new URL(request.url!);
      if (!["tomcat-system-model.invalid", "tomcat-system-mcp.invalid"].includes(url.hostname) || !url.port) {
        response.writeHead(502); response.end("not an acceptance target"); return;
      }
      const parts: Buffer[] = [];
      for await (const part of request) parts.push(Buffer.from(part));
      const body = Buffer.concat(parts);
      let rpcMethod: string | undefined;
      try { rpcMethod = JSON.parse(body.toString()).method; } catch { /* non-JSON request */ }
      requests.push({ method: request.method!, url: url.href, rpcMethod, body: body.toString() });
      const upstream = http.request({ hostname: "127.0.0.1", port: Number(url.port), path: url.pathname + url.search,
        method: request.method, headers: { ...request.headers, host: `127.0.0.1:${url.port}` } }, incoming => {
        response.writeHead(incoming.statusCode!, incoming.headers); incoming.pipe(response);
      });
      upstream.on("error", () => { if (!response.headersSent) response.writeHead(502); response.end(); });
      response.on("close", () => upstream.destroy());
      upstream.end(body);
    })().catch(error => response.destroy(error));
  });
  proxy.on("connection", socket => { connections.add(socket); socket.once("close", () => connections.delete(socket)); });
  const receipt: Record<string, unknown> = { platform: process.platform, node: process.version, image: process.env.ImageOS, imageVersion: process.env.ImageVersion, tomcatRevision: process.env.GITHUB_SHA, status: "failed" };
  try {
    let address = "";
    const deadline = Date.now() + 20_000;
    while (!/^127\.0\.0\.1:\d+$/.test(address)) {
      if (mcpError) throw mcpError;
      if (mcp.exitCode !== null || Date.now() > deadline) throw new Error(`MCP fixture did not bind: ${mcpLog}`);
      address = await fs.readFile(addressFile, "utf8").then(value => value.trim(), () => "");
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    await fs.writeFile(bound, address);
    env.TOMCAT_SYSTEM_PROXY_MCP_URL = `http://${address.replace("127.0.0.1", "tomcat-system-mcp.invalid")}/mcp`;
    await new Promise<void>((resolve, reject) => { proxy.once("error", reject); proxy.listen(0, "127.0.0.1", resolve); });
    const proxyAddress = proxy.address();
    assert.ok(proxyAddress && typeof proxyAddress !== "string");
    receipt.proxy = `127.0.0.1:${proxyAddress.port}`;
    const test = async (name: string, mode: "on" | "off") => {
      let output = "";
      try {
        const result = await runServiceTest(testExe, [name, "--exact", "--ignored", "--nocapture", "--test-threads=1"],
          { cwd: rustRoot, env: { ...env, TOMCAT_SYSTEM_PROXY_ACCEPTANCE: mode }, timeout: 120_000 });
        output = result.stdout + result.stderr;
        assert.match(output, /test result: ok\. 1 passed; 0 failed; 0 ignored;/, "zero matches/skips do not count");
      } catch (error) {
        output += String(error) + String((error as { stdout?: string }).stdout ?? "") + String((error as { stderr?: string }).stderr ?? "");
        throw error;
      } finally { await fs.writeFile(path.join(artifacts, `${name}.log`), output); }
    };
    await withWindowsSystemProxy(receipt.proxy as string, async setEnabled => {
      await setEnabled(false);
      await test("system_proxy_off_cannot_reach_remote_model", "off");
      assert.equal(requests.length, 0, "the disabled proxy must receive no request");
      receipt.disabledControl = "passed";
      await setEnabled(true);
      await test("system_proxy_routes_native_and_acp_model_and_remote_mcp", "on");
      assert.ok(requests.some(request => request.url.includes("tomcat-system-model.invalid") && request.method === "POST"));
      assert.equal(requests.filter(request => request.rpcMethod === "tools/call").length, 2);
      receipt.nativeAndAcp = "passed";
      const windowStart = requests.length;
      const guiEnv = { ...env, TOMCAT_E2E_SHARED_LIFECYCLE: "1", TOMCAT_E2E_SYSTEM_PROXY: "1", TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: artifacts };
      try {
        const gui = await runServiceTest(process.execPath, ["--import", "tsx", path.join(__dirname, "run-vscode-devhost.ts")],
          { cwd: extensionRoot, env: guiEnv, timeout: 600_000 });
        await fs.writeFile(path.join(artifacts, "vscode-launch.log"), gui.stdout + gui.stderr);
      } catch (error) {
        await fs.writeFile(path.join(artifacts, "vscode-launch.log"), String(error) + String((error as { stdout?: string }).stdout ?? "") + String((error as { stderr?: string }).stderr ?? ""));
        throw error;
      }
      assert.ok(requests.slice(windowStart).some(request => request.url.includes("tomcat-system-model.invalid") && request.body?.includes("Write the lifecycle fixture file")), "real window model request must cross the OS proxy");
      receipt.window = "passed";
    });
    receipt.restored = true; // wrapper verifies exact registry values/kinds after restoration.
    receipt.status = "passed";
  } catch (error) { receipt.error = String(error); throw error; }
  finally {
    await Promise.all([
      fs.writeFile(path.join(artifacts, "proxy-requests.json"), JSON.stringify(requests, null, 2)),
      fs.writeFile(path.join(artifacts, "system-proxy-result.json"), JSON.stringify(receipt, null, 2)),
      fs.writeFile(path.join(artifacts, "mcp-server.log"), mcpLog),
    ]);
    for (const socket of connections) socket.destroy();
    if (proxy.listening) await new Promise<void>((resolve, reject) => proxy.close(error => error ? reject(error) : resolve()));
    if (mcp.exitCode === null && mcp.signalCode === null && !mcpError) { const exited = once(mcp, "exit"); mcp.kill(); await exited; }
  }
}
if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });
