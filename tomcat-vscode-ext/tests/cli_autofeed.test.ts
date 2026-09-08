import { execFile, spawn } from "node:child_process";
import * as fs from "node:fs/promises";
import * as http from "node:http";
import * as path from "node:path";
import { promisify } from "node:util";
import { expect, it, vi } from "vitest";
import { ensureTomcatBinary, setupServeFixture, sseDone, sseFinish, sseToolCall, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const exec = promisify(execFile);
async function within<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try { return await Promise.race([promise, new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`CLI fixture deadline ${ms}ms`)), ms);
  })]); } finally { clearTimeout(timer); }
}

it.each([true, false])("background completion before EOF=%s preserves notification/exit ordering", async (completeFirst) => {
  let fixture: Awaited<ReturnType<typeof setupServeFixture>> | undefined;
  let child: ReturnType<typeof spawn> | undefined;
  let stdout = "", stderr = "", taskId = "";
  let mainCalls = 0;
  let completionRequest: unknown;
  let serverFailure: unknown;
  let releaseResponse!: () => void;
  const responseGate = new Promise<void>((resolve) => { releaseResponse = resolve; });
  const server = http.createServer((request, response) => {
    void (async () => {
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      const body = JSON.parse(Buffer.concat(chunks).toString());
      response.setHeader("Connection", "close");
      if (body.stream === false) {
        response.setHeader("Content-Type", "application/json");
        response.end(JSON.stringify({ id: "title", choices: [{ index: 0, message: { role: "assistant", content: "Fixture" }, finish_reason: "stop" }] }));
        return;
      }
      mainCalls++;
      console.log(`mock model request ${mainCalls}: ${body.messages.map((message: { role: string }) => message.role).join(", ")}`);
      response.setHeader("Content-Type", "text/event-stream");
      if (mainCalls === 1) {
        response.end(sseToolCall("background", "bash", JSON.stringify({
          command: "printf '%s' $$ > background.pid; while [ ! -f release ]; do sleep 0.05; done; printf BG_DONE",
          cwd: fixture!.workspacePath, run_in_background: true,
        })).body + sseFinish("tool_calls").body + sseDone().body);
        return;
      }
      if (mainCalls === 2) {
        const tool = body.messages.findLast((message: { role: string }) => message.role === "tool");
        expect(tool, "second main request must include background bash result").toBeDefined();
        taskId = JSON.parse(tool.content).taskId;
        expect(taskId).toBeTruthy();
        await responseGate;
        response.end(`data: ${JSON.stringify({ choices: [{ delta: { content: "FIRST_DONE" }, finish_reason: "stop" }] })}\n\n${sseDone().body}`);
        return;
      }
      const signal = body.messages.find((message: { role: string; content: unknown }) =>
        message.role === "user" && typeof message.content === "string" && message.content.includes(`<background-task-finished task_id="${taskId}"`));
      expect(completeFirst).toBe(true);
      expect(signal, "expected actual task completion message, not system prompt instructions").toBeTruthy();
      expect(signal.content).toContain("BG_DONE");
      completionRequest = body;
      response.end(`data: ${JSON.stringify({ choices: [{ delta: { content: "AUTOFEED_OK" }, finish_reason: "stop" }] })}\n\n${sseDone().body}`);
    })().catch((error) => { serverFailure ??= error; console.error("mock response failed", error); response.destroy(); });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as { port: number }).port;
  let closed: Promise<number | null> | undefined;
  try {
    fixture = await setupServeFixture(`http://127.0.0.1:${port}`);
    const binary = await ensureTomcatBinary();
    const env = { ...fixture.env, TOMCAT__PRIMITIVE__AUTO_CONFIRM: "true", TOMCAT__LLM__TITLE_MODEL: "gpt-5.4", RUST_LOG: "tomcat=info" };
    await exec(binary, ["workspace", "add", fixture.workspacePath], { cwd: fixture.workspacePath, env, timeout: 15_000 });
    child = spawn(binary, ["code"], { cwd: fixture.workspacePath, env, stdio: "pipe", detached: process.platform !== "win32" });
    closed = new Promise((resolve, reject) => { child!.once("close", resolve); child!.once("error", reject); });
    child.stdout!.on("data", (chunk) => { stdout = (stdout + chunk).slice(-2_000_000); });
    child.stderr!.on("data", (chunk) => { stderr = (stderr + chunk).slice(-2_000_000); });
    child.stdin!.on("error", () => {});
    child.stdin!.write("Run the fixture background command.\n");
    await vi.waitFor(() => { if (serverFailure) throw serverFailure; expect(taskId, stderr).toBeTruthy(); }, { timeout: 20_000 });
    if (completeFirst) {
      await fs.writeFile(path.join(fixture.workspacePath, "release"), "go");
      await vi.waitFor(() => expect(stderr).toContain("queued for next turn"), { timeout: 10_000 });
      releaseResponse();
      await vi.waitFor(() => { if (serverFailure) throw serverFailure; expect(stdout, stderr).toContain("AUTOFEED_OK"); }, { timeout: 15_000 });
      child.stdin!.end();
      expect(completionRequest).toBeTruthy();
    } else {
      child.stdin!.end();
      releaseResponse();
    }
    expect(await within(closed, 15_000), stderr).toBe(0);
    expect(serverFailure).toBeUndefined();
    expect(mainCalls).toBe(completeFirst ? 3 : 2);
    if (!completeFirst) {
      expect(completionRequest).toBeUndefined();
      expect(stdout).not.toContain("AUTOFEED_OK");
      expect(stderr).not.toContain("queued for next turn");
    }
  } catch (error) {
    console.error("CLI autofeed failure", { stdout: stdout.slice(-4000), stderrStart: stderr.slice(0, 9000), stderrEnd: stderr.slice(-2000) });
    throw error;
  } finally {
    releaseResponse();
    if (child && child.exitCode === null && child.signalCode === null) {
      if (process.platform !== "win32" && child.pid) { try { process.kill(-child.pid, "SIGKILL"); } catch { /* already exited */ } }
      child.kill("SIGKILL");
    }
    if (closed) await within(closed, 5_000);
    // Background bash has its own process group. Stop only the PID that this
    // fixture wrote, never a name-based process search or a user's task.
    if (fixture) {
      const pid = Number(await fs.readFile(path.join(fixture.workspacePath, "background.pid"), "utf8").catch(() => ""));
      if (pid > 1 && process.platform !== "win32") { try { process.kill(-pid, "SIGKILL"); } catch { /* already exited */ } }
    }
    server.closeAllConnections();
    await within(new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())), 3_000);
    if (fixture) await fixture.cleanup();
  }
}, 90_000);
