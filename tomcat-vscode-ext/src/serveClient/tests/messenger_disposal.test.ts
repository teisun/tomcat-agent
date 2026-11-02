import { afterEach, describe, expect, it, vi } from "vitest";
import { TomcatMessenger } from "../TomcatMessenger";
import type { SharedConnection, SharedConnectionFactory } from "../sharedConnection";
import { createConnectionFactory, FakeConnection, FAKE_SERVICE } from "./fakes";

async function fixture(socket = new FakeConnection()) {
  const connectFactory = vi.fn(createConnectionFactory(socket));
  const messenger = new TomcatMessenger({ executable: "fake", connectFactory });
  await messenger.start();
  return { socket, messenger, connectFactory };
}

afterEach(() => vi.useRealTimers());

describe("TomcatMessenger shared connection disposal", () => {
  it("is idempotent without starting an unused messenger", async () => {
    const connectFactory = vi.fn();
    const messenger = new TomcatMessenger({ executable: "unused", connectFactory });
    const first = messenger.disposeAsync();
    expect(messenger.disposeAsync()).toBe(first);
    messenger.dispose();
    await first;
    expect(connectFactory).not.toHaveBeenCalled();
  });

  it("half-closes and waits for the peer, without stopping the service", async () => {
    const socket = new FakeConnection();
    socket.autoClose = false;
    const { messenger } = await fixture(socket);
    const management = vi.spyOn(messenger, "runServiceAction");
    const exited = vi.fn();
    messenger.onExit(exited);
    const closed = vi.fn();
    const disposal = messenger.disposeAsync().then(closed);
    await Promise.resolve();
    expect(socket.writableEnded).toBe(true);
    expect(socket.destroyed).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    socket.close();
    await disposal;
    expect(closed).toHaveBeenCalledOnce();
    expect(exited).not.toHaveBeenCalled();
    expect(management).not.toHaveBeenCalled();
  });

  it("retains old generations until close and ignores their late frames", async () => {
    const first = new FakeConnection();
    first.autoClose = false;
    const second = new FakeConnection();
    const connections = [first, second];
    const connectFactory: SharedConnectionFactory = options => createConnectionFactory(connections.shift()!)(options);
    const messenger = new TomcatMessenger({ executable: "fake", connectFactory });
    const events = vi.fn();
    messenger.onEvent(events);
    await messenger.start();
    await messenger.restart();
    first.emitStdout('{"type":"agent_idle","sessionId":"old"}\n');
    expect(messenger.isRunning).toBe(true);
    const completed = vi.fn();
    const disposal = messenger.disposeAsync().then(completed);
    await Promise.resolve();
    expect(completed).not.toHaveBeenCalled();
    first.close();
    await disposal;
    expect(events).not.toHaveBeenCalled();
  });

  it("rejects pending requests once on a socket error", async () => {
    const { socket, messenger } = await fixture();
    const pending = expect(messenger.request({ type: "get_state" })).rejects.toThrow("pipe broke");
    const exited = vi.fn();
    messenger.onExit(exited);
    socket.fail(new Error("pipe broke"));
    await pending;
    await messenger.disposeAsync();
    expect(exited).toHaveBeenCalledOnce();
    expect(messenger.isRunning).toBe(false);
  });

  it("also awaits a previous synchronous stop/dispose", async () => {
    const socket = new FakeConnection();
    socket.autoClose = false;
    const { messenger } = await fixture(socket);
    messenger.stop();
    messenger.dispose();
    const disposal = messenger.disposeAsync();
    expect(messenger.disposeAsync({ timeoutMs: 1 })).toBe(disposal);
    socket.close();
    await disposal;
  });

  it("reports unconfirmed cleanup on timeout instead of killing the service", async () => {
    vi.useFakeTimers();
    const socket = new FakeConnection();
    socket.autoClose = false;
    const { messenger } = await fixture(socket);
    const management = vi.spyOn(messenger, "runServiceAction");
    const failure = expect(messenger.disposeAsync({ timeoutMs: 100 })).rejects.toThrow("service cleanup is unconfirmed");
    await vi.advanceTimersByTimeAsync(100);
    await failure;
    expect(socket.destroyed).toBe(true);
    expect(management).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("closes a connection that finishes opening after disposal", async () => {
    const socket = new FakeConnection();
    let connected!: (connection: SharedConnection) => void;
    const connectFactory = vi.fn(() => new Promise<SharedConnection>(resolve => { connected = resolve; }));
    const messenger = new TomcatMessenger({ executable: "fake", connectFactory });
    const starting = messenger.start();
    await Promise.resolve();
    const disposal = messenger.disposeAsync();
    connected({ socket, info: FAKE_SERVICE });
    await starting;
    await disposal;
    expect(socket.destroyed).toBe(true);
  });

  it("does not send requests whose deadline expired while connecting", async () => {
    vi.useFakeTimers();
    const socket = new FakeConnection();
    let connected!: (connection: SharedConnection) => void;
    const messenger = new TomcatMessenger({ executable: "fake", connectFactory: () => new Promise(resolve => { connected = resolve; }) });
    const failure = expect(messenger.request({ id: "old", type: "prompt", text: "do not run" }, 50)).rejects.toThrow("Timed out");
    await vi.advanceTimersByTimeAsync(50);
    await failure;
    connected({ socket, info: FAKE_SERVICE });
    await messenger.start();
    expect(socket.readStdin()).toBe("");
    await messenger.disposeAsync();
  });

  it("validates timeout without disposing a healthy connection", async () => {
    const { messenger } = await fixture();
    for (const timeoutMs of [0, -1, NaN, Infinity]) await expect(messenger.disposeAsync({ timeoutMs })).rejects.toThrow(RangeError);
    expect(messenger.isRunning).toBe(true);
    await messenger.disposeAsync();
  });
});
