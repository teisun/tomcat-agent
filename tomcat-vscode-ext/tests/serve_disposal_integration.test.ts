import * as path from "node:path";
import { mkdir, readFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { expect, it } from "vitest";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { initializeServe } from "../src/serveClient/initialize";
import type { ServeEvent } from "../src/serveClient/wire";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { ensureTomcatBinary, setupServeFixture, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, sseToolCall, waitForEvent } from "./serveTestUtils";

const execFileAsync = promisify(execFile);

it("two windows share a real service; reload preserves the current session and explicit stop stays stopped", async () => {
  const fixture = await setupServeFixture("http://127.0.0.1:1");
  const executable = await ensureTomcatBinary();
  const other = path.join(fixture.homePath, "other-project");
  await mkdir(other);
  const a = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: fixture.env });
  const b = new TomcatMessenger({ executable, cwd: other, env: fixture.env });
  const reloaded = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: fixture.env });
  const probe = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: fixture.env });
  const status = async () => JSON.parse((await execFileAsync(executable, ["serve", "status"], { cwd: fixture.workspacePath, env: fixture.env })).stdout);
  try {
    const [first, second] = await Promise.all([initializeServe(a), initializeServe(b)]);
    expect(a.pid).toBe(b.pid);
    expect(first.sessionId).toBeTruthy();
    expect(second.sessionId).not.toBe(first.sessionId);
    const [replyA, replyB] = await Promise.all([
      a.request({ type: "get_state", id: "same-id", sessionId: first.sessionId }),
      b.request({ type: "get_state", id: "same-id", sessionId: second.sessionId }),
    ]);
    expect(replyA).toMatchObject({ success: true, sessionId: first.sessionId });
    expect(replyB).toMatchObject({ success: true, sessionId: second.sessionId });
    const listB = await new SessionRouter(b, () => other).listSessions("disk");
    expect(listB.sessions.map(s => s.sessionId)).toEqual([second.sessionId]);
    expect(await b.request({ type: "switch_session", sessionId: first.sessionId! })).toMatchObject({ success: false, error: "unknown_session" });
    const instance = (await status()).instanceId;
    await a.disposeAsync();
    expect(await status()).toMatchObject({ running: true, instanceId: instance });
    const restored = await initializeServe(reloaded);
    expect(restored.sessionId).toBe(first.sessionId);
    expect(reloaded.pid).toBe(b.pid);
    expect((await new SessionRouter(reloaded, () => fixture.workspacePath).listSessions("disk")).sessions).toHaveLength(1);
    expect(await b.request({ type: "get_state", sessionId: second.sessionId })).toMatchObject({ success: true });
    await b.runServiceAction("stop");
    expect(await status()).toMatchObject({ running: false });
    await expect(probe.start({ allowStart: false })).rejects.toThrow("unavailable");
    expect(await status()).toMatchObject({ running: false });
  } finally {
    await Promise.all([a, b, reloaded, probe].map(messenger => messenger.disposeAsync()));
    await fixture.cleanup();
  }
}, 120_000);

it.skipIf(process.platform === "win32")("window EOF stops its real shell, not another window or the shared service", async () => {
  let release!: () => void;
  const hold = new Promise<void>(resolve => { release = resolve; });
  // Relative tool paths also prove that the client's cwd reaches the executor.
  const command = 'printf "%s" "$WINDOW_LABEL" > environment.txt; echo $$ > worker.pid; while true; do echo tick >> heartbeat.txt; sleep 0.05; done';
  const model = await spawnScriptedOpenAiStreamServer([
    { parts: [sseToolCall("worker", "bash", JSON.stringify({ command, cwd: "", foreground_wait_ms: 8000, run_in_background: true })), sseFinish("tool_calls"), sseDone()] },
    { parts: [{ body: "", waitFor: hold }, sseDelta("cancelled caller must not continue"), sseFinish("stop"), sseDone()] },
    { parts: [sseDelta("other window is healthy"), sseFinish("stop"), sseDone()] },
  ]);
  const fixture = await setupServeFixture(model.baseUrl);
  const executable = await ensureTomcatBinary();
  const other = path.join(fixture.homePath, "other-project");
  await mkdir(other);
  const b = new TomcatMessenger({ executable, cwd: other, env: { ...fixture.env, WINDOW_LABEL: "starter-B" } });
  const a = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: { ...fixture.env, WINDOW_LABEL: "window-A" } });
  const observed: ServeEvent[] = [];
  a.onEvent(event => observed.push(event));
  try {
    const second = await initializeServe(b); // B starts the service first.
    const first = await initializeServe(a);
    expect(await a.request({ type: "prompt", sessionId: first.sessionId, text: "start the background worker" })).toMatchObject({ success: true });
    const read = (file: string) => readFile(path.join(fixture.workspacePath, file), "utf8");
    await expect.poll(() => read("environment.txt").catch(() => JSON.stringify({ requests: model.capturedNonTitleRequests().length, events: observed.slice(-6) })), { timeout: 10_000 }).toBe("window-A");
    await expect.poll(() => model.capturedNonTitleRequests().length, { timeout: 10_000 }).toBe(2);
    const pid = Number((await read("worker.pid")).trim());
    expect(pid).toBeGreaterThan(0);
    await a.disposeAsync();
    release();
    expect(() => process.kill(pid, 0)).toThrow();
    const stopped = await read("heartbeat.txt");
    const idle = waitForEvent(b, event => event.type === "agent_idle" && event.sessionId === second.sessionId);
    const response = b.request({ type: "prompt", sessionId: second.sessionId, text: "continue in this window" });
    const [reply, events] = await Promise.all([response, idle]);
    expect(reply.success).toBe(true);
    expect(JSON.stringify(events)).toContain("other window is healthy");
    expect(await read("heartbeat.txt")).toBe(stopped);
  } finally {
    release();
    await Promise.all([a.disposeAsync(), b.disposeAsync()]);
    await fixture.cleanup();
    await model.close();
  }
}, 120_000);
