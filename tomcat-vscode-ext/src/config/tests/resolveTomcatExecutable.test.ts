import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it, vi } from "vitest";

import { TEST_PATH_ENV } from "../../constants";
import { resolveTomcatExecutable } from "../resolveTomcatExecutable";

describe("resolveTomcatExecutable", () => {
  it("prefers the test override when present", async () => {
    const resolved = await resolveTomcatExecutable({
      env: {
        [TEST_PATH_ENV]: "/tmp/fake-tomcat",
      },
    });

    expect(resolved).toEqual({
      executable: "/tmp/fake-tomcat",
      found: true,
      source: "test-env",
    });
  });

  it("keeps an explicit configured path", async () => {
    const resolved = await resolveTomcatExecutable({
      commandRunner: async () => "",
      configuredPath: "/custom/bin/tomcat",
      fileExists: async (targetPath) => targetPath === "/custom/bin/tomcat",
      pathWasConfigured: true,
    });

    expect(resolved).toEqual({
      executable: "/custom/bin/tomcat",
      found: true,
      source: "config",
    });
  });

  it("prefers the user command over an older PATH entry without spawning a locator", async () => {
    const installed = path.join("/Users/tester", ".local", "bin", "tomcat");
    const commandRunner = vi.fn(async () => { throw new Error("must not run a locator"); });
    const resolved = await resolveTomcatExecutable({
      env: { HOME: "/Users/tester", PATH: "/old/bin" },
      commandRunner,
      fileExists: async () => true,
      platform: "darwin",
    });
    expect(resolved).toEqual({ executable: installed, found: true, source: "user-bin" });
    expect(commandRunner).not.toHaveBeenCalled();
  });

  it("falls through to external PATH discovery when the user command is absent", async () => {
    const resolved = await resolveTomcatExecutable({
      commandRunner: async () => { throw new Error("unexpected command"); },
      env: { PATH: "/missing:/usr/local/bin" },
      fileExists: async (candidate) => candidate === "/usr/local/bin/tomcat",
      platform: "darwin",
    });

    expect(resolved).toEqual({
      executable: "/usr/local/bin/tomcat",
      found: true,
      source: "process-path",
    });
  });

  it("falls back to a login shell lookup when the process path misses tomcat", async () => {
    const resolved = await resolveTomcatExecutable({
      commandRunner: async (command, args) => {
        if (args[1] === "command -v tomcat") {
          return "/opt/homebrew/bin/tomcat\n";
        }
        throw new Error(`unexpected command ${command} ${args.join(" ")}`);
      },
      env: { SHELL: "/bin/zsh" },
      platform: "darwin",
      shellPath: "/bin/zsh",
    });

    expect(resolved).toEqual({
      executable: "/opt/homebrew/bin/tomcat",
      found: true,
      source: "shell-path",
    });
  });

  it("falls back to common install paths before giving up", async () => {
    const installed = path.join("/Users/tester", ".local", "bin", "tomcat");
    const resolved = await resolveTomcatExecutable({
      commandRunner: async () => {
        throw new Error("not found");
      },
      env: {
        HOME: "/Users/tester",
      },
      fileExists: async (targetPath) => targetPath === installed,
      platform: "darwin",
    });

    expect(resolved).toEqual({
      executable: installed,
      found: true,
      source: "user-bin",
    });
  });

  it("does not replace an invalid explicit override with the user command", async () => {
    const resolved = await resolveTomcatExecutable({
      env: { HOME: "/test-home" }, configuredPath: "/invalid/custom-tomcat", pathWasConfigured: true,
      fileExists: async candidate => candidate !== "/invalid/custom-tomcat",
    });
    expect(resolved).toEqual({ executable: "/invalid/custom-tomcat", found: false, source: "config" });
  });

  it("uses HOME ahead of USERPROFILE like the CLI installer", async () => {
    const executable = path.join("/owned-home", ".local", "bin", "tomcat.exe");
    expect(await resolveTomcatExecutable({
      env: { HOME: "/owned-home", USERPROFILE: "/other-home" }, platform: "win32",
      fileExists: async candidate => candidate === executable,
    })).toEqual({ executable, found: true, source: "user-bin" });
  });

  it("returns the default command when discovery fails", async () => {
    const resolved = await resolveTomcatExecutable({
      commandRunner: async () => {
        throw new Error("not found");
      },
      fileExists: async () => false,
    });

    expect(resolved).toEqual({
      executable: "tomcat",
      found: false,
      source: "default",
    });
  });

  it("finds a real executable in a non-ASCII PATH directory without a locator process", async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-path-"));
    const directory = path.join(root, "tomcat 汤姆猫");
    const executable = path.join(directory, process.platform === "win32" ? "tomcat.exe" : "tomcat");
    const commandRunner = vi.fn(async () => { throw new Error("must not spawn a locator"); });
    try {
      await fs.mkdir(directory);
      await fs.writeFile(executable, "fixture", { mode: 0o755 });
      const resolved = await resolveTomcatExecutable({
        env: { PATH: [path.join(root, "missing"), `"${directory}"`].join(path.delimiter) },
        commandRunner,
      });
      expect(resolved).toEqual({ executable, found: true, source: "process-path" });
      expect(commandRunner).not.toHaveBeenCalled();
    } finally {
      await fs.rm(root, { recursive: true, force: true });
    }
  });

  it.skipIf(process.platform === "win32")("keeps executable symlinks usable on POSIX", async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-symlink-"));
    const executable = path.join(root, "tomcat");
    try {
      await fs.writeFile(path.join(root, "target"), "fixture", { mode: 0o755 });
      await fs.symlink("target", executable);
      expect(await resolveTomcatExecutable({ env: { PATH: root } })).toEqual({
        executable, found: true, source: "process-path",
      });
    } finally { await fs.rm(root, { recursive: true, force: true }); }
  });

  it("skips a PATH directory named tomcat and keeps looking for a regular executable", async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-directory-"));
    const name = process.platform === "win32" ? "tomcat.exe" : "tomcat";
    const decoy = path.join(root, "first", name);
    const executable = path.join(root, "second", name);
    const commandRunner = vi.fn(async () => { throw new Error("must not spawn a locator"); });
    try {
      await fs.mkdir(decoy, { recursive: true });
      await fs.mkdir(path.dirname(executable));
      await fs.writeFile(executable, "fixture", { mode: 0o755 });
      const resolved = await resolveTomcatExecutable({
        env: { PATH: [path.dirname(decoy), path.dirname(executable)].join(path.delimiter) }, commandRunner,
      });
      expect(resolved).toEqual({ executable, found: true, source: "process-path" });
      expect(commandRunner).not.toHaveBeenCalled();
    } finally { await fs.rm(root, { recursive: true, force: true }); }
  });

  describe("on win32", () => {
    it("accepts a user-installed tomcat.exe without the POSIX execute bit", async () => {
      const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-resolve-"));
      const executable = path.join(dir, ".local", "bin", "tomcat.exe");
      try {
        await fs.mkdir(path.dirname(executable), { recursive: true });
        await fs.writeFile(executable, "MZ", { mode: 0o644 });
        await expect(resolveTomcatExecutable({ env: { HOME: dir }, platform: "win32" }))
          .resolves.toEqual({ executable, found: true, source: "user-bin" });
      } finally {
        await fs.rm(dir, { force: true, recursive: true });
      }
    });

    it("scans PATH in order and skips the POSIX login-shell lookup", async () => {
      const commandRunner = vi.fn(async () => { throw new Error("unexpected command"); });
      const installed = "C:\\External\\tomcat.exe";
      const checked: string[] = [];
      const resolved = await resolveTomcatExecutable({
        commandRunner,
        env: { PATH: '; "C:\\missing dir" ;C:\\External;C:\\other' },
        fileExists: async (candidate) => { checked.push(candidate); return candidate === installed; },
        platform: "win32",
      });

      expect(resolved).toEqual({ executable: installed, found: true, source: "process-path" });
      expect(checked).toEqual(["C:\\missing dir\\tomcat.exe", installed]);
      expect(commandRunner).not.toHaveBeenCalled();
    });

    it("falls back to tomcat.exe under USERPROFILE when PATH misses", async () => {
      const installed = path.join("C:\\Users\\tester", ".local", "bin", "tomcat.exe");
      const resolved = await resolveTomcatExecutable({
        commandRunner: async () => {
          throw new Error("not found");
        },
        env: { PATH: "C:\\missing;C:\\also-missing", USERPROFILE: "C:\\Users\\tester" },
        fileExists: async (targetPath) => targetPath === installed,
        platform: "win32",
      });

      expect(resolved).toEqual({ executable: installed, found: true, source: "user-bin" });
    });
  });
});
