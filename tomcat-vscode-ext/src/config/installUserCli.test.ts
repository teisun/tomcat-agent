import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installUserCli } from "./installUserCli";

const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map(root => fs.rm(root, { recursive: true, force: true }))); });
async function bundled() {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cli-install-中文 "));
  roots.push(root);
  const executable = path.join(root, "tomcat.exe");
  await fs.writeFile(executable, "installation material, never executed by unit tests");
  return executable;
}

describe("automatic user CLI installation", () => {
  it("uses only self-install with its native timeout and preserves raw output", async () => {
    const executable = await bundled();
    const run = vi.fn().mockResolvedValue({ stdout: "file unchanged\nPATH ready\n", stderr: "raw diagnostic\n" });
    const log = vi.fn();
    await installUserCli(executable, log, run);
    expect(run).toHaveBeenCalledExactlyOnceWith(executable, ["self-install", "--add-to-path"], {
      encoding: "utf8", windowsHide: true, timeout: 30_000,
    });
    expect(log).toHaveBeenCalledWith("file unchanged\nPATH ready\n");
    expect(log).toHaveBeenCalledWith("raw diagnostic\n");
    expect(log.mock.calls.flat().join("\n")).toContain("exited with code 0");
  });
  it("skips pure extensions instead of starting another runtime", async () => {
    const executable = await bundled();
    await fs.unlink(executable);
    const run = vi.fn(); const log = vi.fn();
    await installUserCli(executable, log, run);
    expect(run).not.toHaveBeenCalled();
    expect(log.mock.calls.flat().join("\n")).toContain("automatic installation skipped");
  });
  it.each([
    { code: 1, message: "Command failed: <exe> self-install --add-to-path\nmanual PATH steps", stdout: "file installed", stderr: "manual PATH steps" },
    { code: "ENOENT", message: "cannot spawn" },
    { code: null, signal: "SIGTERM", killed: true, message: "installation timed out", stdout: "copy started" },
  ])("logs failures without rejecting activation or inventing success: $message", async failure => {
    const executable = await bundled(); const log = vi.fn();
    const run = vi.fn().mockRejectedValue(failure);
    await expect(installUserCli(executable, log, run)).resolves.toBeUndefined();
    const text = log.mock.calls.flat().join("\n");
    expect(text).toContain(failure.message.split(/\r?\n/, 1)[0]);
    expect(text).not.toContain("exited with code 0");
    if ("stdout" in failure) expect(text).toContain(failure.stdout);
    if ("stderr" in failure) {
      expect(log).toHaveBeenCalledWith(failure.stderr);
      expect(text.match(/manual PATH steps/g)).toHaveLength(1);
    }
    if ("killed" in failure) expect(text).toContain("killed=true");
  });
});
