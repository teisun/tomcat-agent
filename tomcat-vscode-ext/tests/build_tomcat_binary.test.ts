import { EventEmitter } from "node:events";
import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ spawn: vi.fn(), execFile: vi.fn(), stat: vi.fn() }));
vi.mock("node:child_process", () => ({ spawn: mocks.spawn, execFile: mocks.execFile }));
vi.mock("node:fs/promises", () => ({ stat: mocks.stat }));
vi.mock("../scripts/resolveCargoCommand", () => ({ resolveCargoCommand: () => "cargo" }));
import { buildTomcatBinary } from "../scripts/buildTomcatBinary";

let build: EventEmitter;
beforeEach(() => {
  vi.clearAllMocks();
  build = new EventEmitter();
  mocks.spawn.mockReturnValue(build);
  mocks.stat.mockResolvedValue({ isFile: () => true });
});

it("waits for the fresh CLI to launch before releasing tests, using only --version", async () => {
  let ready = false;
  const result = buildTomcatBinary().then(value => { ready = true; return value; });
  expect(mocks.execFile).not.toHaveBeenCalled();
  build.emit("close", 0, null);
  await vi.waitFor(() => expect(mocks.execFile).toHaveBeenCalledOnce());
  expect(ready).toBe(false);
  const [binary, args, options, callback] = mocks.execFile.mock.calls[0];
  expect(args).toEqual(["--version"]);
  expect(options).toEqual({ timeout: 120_000, windowsHide: true });
  expect(mocks.spawn).toHaveBeenCalledWith("cargo", ["build", "--bin", "tomcat"], expect.any(Object));
  callback(null, "tomcat 0.1.81\n");
  await expect(result).resolves.toBe(binary);
});

it("does not launch a stale CLI when compilation fails", async () => {
  const result = buildTomcatBinary();
  const rejected = expect(result).rejects.toThrow("cargo build failed (1)");
  build.emit("close", 1, null);
  await rejected;
  expect(mocks.execFile).not.toHaveBeenCalled();
});

it("preserves a first-launch failure instead of announcing readiness", async () => {
  const failure = Object.assign(new Error("CLI first launch timed out"), { code: "ETIMEDOUT", killed: true });
  const result = buildTomcatBinary();
  const rejected = expect(result).rejects.toBe(failure);
  build.emit("close", 0, null);
  await vi.waitFor(() => expect(mocks.execFile).toHaveBeenCalledOnce());
  mocks.execFile.mock.calls[0][3](failure, "");
  await rejected;
});
