import { describe, expect, it } from "vitest";
import * as path from "node:path";
import { runServiceTest } from "../scripts/run-windows-system-proxy";

const options = () => ({ cwd: process.cwd(), env: { ...process.env }, timeout: 5_000 });

describe("awaited Windows acceptance process boundary", () => {
  it("captures both streams and waits for successful exit", async () => {
    const result = await runServiceTest(process.execPath, ["-e", "process.stdout.write('out'); process.stderr.write('err');"], options());
    expect(result).toEqual({ stdout: "out", stderr: "err" });
  });

  it("retains failure output and rejects a nonzero exit", async () => {
    await expect(runServiceTest(process.execPath, ["-e", "console.error('fixture failure'); process.exitCode = 7;"], options()))
      .rejects.toMatchObject({ message: expect.stringContaining("(7, null)"), stderr: "fixture failure\n" });
  });

  it("rejects and reaps a process that exceeds the deadline", async () => {
    await expect(runServiceTest(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { ...options(), timeout: 250 }))
      .rejects.toThrow("Acceptance command failed");
  });

  it("reports executable launch errors instead of hanging", async () => {
    await expect(runServiceTest(path.join(process.cwd(), "missing-acceptance-executable"), [], options()))
      .rejects.toMatchObject({ code: "ENOENT" });
  });
});
