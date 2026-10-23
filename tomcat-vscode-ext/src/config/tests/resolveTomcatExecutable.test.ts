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
      bundledPath: "/bundled/tomcat",
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

  it("prefers the bundled executable after explicit config is ruled out", async () => {
    const resolved = await resolveTomcatExecutable({
      bundledPath: "/extension/bin/tomcat",
      commandRunner: async () => {
        throw new Error("should not reach PATH when bundled exists");
      },
      fileExists: async (targetPath) => targetPath === "/extension/bin/tomcat",
    });

    expect(resolved).toEqual({
      executable: "/extension/bin/tomcat",
      found: true,
      source: "bundled",
    });
  });

  it("falls through to PATH discovery when the bundled executable is absent", async () => {
    const resolved = await resolveTomcatExecutable({
      bundledPath: "/extension/bin/tomcat",
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
      source: "common-path",
    });
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

  describe("on win32", () => {
    it("accepts a bundled tomcat.exe without the POSIX execute bit", async () => {
      const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-resolve-"));
      const bundledPath = path.join(dir, "tomcat.exe");
      try {
        await fs.writeFile(bundledPath, "MZ", { mode: 0o644 });

        await expect(
          resolveTomcatExecutable({ bundledPath, env: {}, platform: "win32" }),
        ).resolves.toEqual({ executable: bundledPath, found: true, source: "bundled" });
      } finally {
        await fs.rm(dir, { force: true, recursive: true });
      }
    });

    it("scans PATH in order and skips the POSIX login-shell lookup", async () => {
      const commandRunner = vi.fn(async () => { throw new Error("unexpected command"); });
      const installed = "C:\\Users\\tester\\.local\\bin\\tomcat.exe";
      const checked: string[] = [];
      const resolved = await resolveTomcatExecutable({
        commandRunner,
        env: { PATH: '; "C:\\missing dir" ;C:\\Users\\tester\\.local\\bin;C:\\other', USERPROFILE: "C:\\Users\\tester" },
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

      expect(resolved).toEqual({ executable: installed, found: true, source: "common-path" });
    });
  });
});
