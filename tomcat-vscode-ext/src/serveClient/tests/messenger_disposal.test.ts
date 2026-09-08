import { spawn } from "node:child_process";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function fixture(child = new FakeChildProcess()) {
  const messenger = new TomcatMessenger({ executable: "fake", spawnFactory: createSpawnFactory(child) });
  messenger.start();
  return { child, messenger };
}

afterEach(() => vi.useRealTimers());

describe("TomcatMessenger awaited disposal", () => {
  it("is idempotent without starting an unused messenger", async () => {
    const spawnFactory = vi.fn();
    const messenger = new TomcatMessenger({ executable: "unused", spawnFactory });
    const first = messenger.disposeAsync();
    expect(messenger.disposeAsync()).toBe(first);
    messenger.dispose();
    await first;
    expect(spawnFactory).not.toHaveBeenCalled();
  });

  it("waits for close, not just exit or killed", async () => {
    vi.useFakeTimers();
    const child = new FakeChildProcess();
    child.autoClose = false;
    const { messenger } = fixture(child);
    const exited = vi.fn();
    messenger.onExit(exited);
    const closed = vi.fn();
    const disposal = messenger.disposeAsync().then(closed);
    await vi.advanceTimersByTimeAsync(500);
    expect(child.killed).toBe(true);
    expect(child.exitCode).toBe(0);
    expect(closed).not.toHaveBeenCalled();
    expect(exited).not.toHaveBeenCalled(); // intentional disposal is not reconnect-worthy
    child.emitStderr("late drain");
    child.close();
    await disposal;
    expect(closed).toHaveBeenCalledOnce();
  });

  it("retains an externally exited child until its pipes close", async () => {
    const { child, messenger } = fixture();
    child.autoClose = false;
    child.exitCode = 0;
    child.emit("exit", 0, null);
    expect(messenger.isRunning).toBe(false);
    let closed = false;
    const disposal = messenger.disposeAsync().then(() => { closed = true; });
    await Promise.resolve();
    expect(closed).toBe(false);
    child.close();
    await disposal;
  });

  it("waits for prior restarted generations without stale events changing the current one", async () => {
    const first = new FakeChildProcess();
    first.autoClose = false;
    const second = new FakeChildProcess();
    const spawnFactory = vi.fn()
      .mockReturnValueOnce(first)
      .mockReturnValueOnce(second) as unknown as typeof spawn;
    const messenger = new TomcatMessenger({ executable: "fake", spawnFactory });
    messenger.start();
    messenger.restart();
    expect(messenger.isRunning).toBe(true);
    first.emit("error", new Error("late old error"));
    first.emitStdout('{"type":"agent_idle","sessionId":"old"}\n');
    expect(messenger.isRunning).toBe(true);
    const disposal = messenger.disposeAsync();
    let done = false;
    void disposal.then(() => { done = true; });
    await Promise.resolve();
    expect(done).toBe(false);
    first.close();
    await disposal;
    expect(messenger.isRunning).toBe(false);
  });

  it("rejects pending work immediately but still waits for an errored child's close", async () => {
    const child = new FakeChildProcess();
    child.autoClose = false;
    const { messenger } = fixture(child);
    const pending = expect(messenger.request({ type: "get_state" })).rejects.toThrow("pipe broke");
    const exited = vi.fn();
    messenger.onExit(exited);
    child.fail(new Error("pipe broke"));
    await pending;
    expect(exited).toHaveBeenCalledOnce();
    expect(child.signals).toContain("SIGTERM");
    const disposal = messenger.disposeAsync();
    child.close();
    await disposal;
  });

  it("also awaits a prior synchronous stop/dispose", async () => {
    const child = new FakeChildProcess();
    child.autoClose = false;
    const { messenger } = fixture(child);
    messenger.stop();
    messenger.dispose();
    const disposal = messenger.disposeAsync();
    expect(messenger.disposeAsync({ timeoutMs: 1 })).toBe(disposal);
    child.close();
    await disposal;
    expect(child.signals).toEqual(["SIGTERM"]);
  });

  it("escalates even when killed=true only means SIGTERM was sent", async () => {
    vi.useFakeTimers();
    const child = new FakeChildProcess();
    child.kill = (signal) => {
      child.signals.push(signal);
      child.killed = true;
      if (signal === "SIGKILL") {
        child.signalCode = signal;
        child.emit("exit", null, signal);
        child.close();
      }
      return true;
    };
    const { messenger } = fixture(child);
    const disposal = messenger.disposeAsync({ timeoutMs: 100 });
    await vi.advanceTimersByTimeAsync(50);
    await disposal;
    expect(child.signals).toEqual(["SIGTERM", "SIGKILL"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejects on the total deadline when exit never produces close", async () => {
    vi.useFakeTimers();
    const child = new FakeChildProcess();
    child.autoClose = false;
    const { messenger } = fixture(child);
    const failure = expect(messenger.disposeAsync({ timeoutMs: 100 }))
      .rejects.toThrow("timed out closing tomcat serve after 100ms: pid=4242");
    await vi.advanceTimersByTimeAsync(100);
    await failure;
    child.close();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("lets EOF settle the child without sending termination signals", async () => {
    vi.useFakeTimers();
    const { child, messenger } = fixture();
    const disposal = messenger.disposeAsync({ timeoutMs: 2_000 });
    expect(child.signals).toEqual([]);
    child.exitCode = 0;
    child.emit("exit", 0, null);
    child.close();
    await disposal;
    await vi.advanceTimersByTimeAsync(2_000);
    expect(child.signals).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("validates the timeout without disposing the messenger", async () => {
    const { messenger } = fixture();
    for (const timeoutMs of [0, -1, NaN, Infinity]) {
      await expect(messenger.disposeAsync({ timeoutMs })).rejects.toThrow(RangeError);
    }
    expect(messenger.isRunning).toBe(true);
    await messenger.disposeAsync();
  });

  it("waits for a real owned Node process and its stdio to close", async () => {
    let child: ReturnType<typeof spawn> | undefined;
    const messenger = new TomcatMessenger({
      executable: process.execPath,
      spawnFactory: ((_exe, _args, options) => {
        child = spawn(process.execPath, ["-e", "process.stdin.resume()"], options ?? {});
        return child;
      }) as typeof spawn,
    });
    messenger.start();
    await messenger.disposeAsync({ timeoutMs: 2_000 });
    expect(child?.exitCode !== null || child?.signalCode !== null).toBe(true);
    expect(child?.stdout?.destroyed).toBe(true);
    expect(child?.stderr?.destroyed).toBe(true);
  });
});
