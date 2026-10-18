import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

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
      commandRunner: async (command) => {
        if (command === "which") {
          return "/usr/local/bin/tomcat\n";
        }
        throw new Error("unexpected command");
      },
      fileExists: async () => false,
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
        if (command === "which") {
          throw new Error("not found");
        }
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

    it("finds tomcat with where.exe and skips the POSIX login-shell lookup", async () => {
      const commands: string[] = [];
      const resolved = await resolveTomcatExecutable({
        commandRunner: async (command, args) => {
          commands.push(`${command} ${args.join(" ")}`);
          if (command === "where.exe") {
            return "C:\\Users\\tester\\.local\\bin\\tomcat.exe\r\nC:\\other\\tomcat.exe\r\n";
          }
          throw new Error("unexpected command");
        },
        env: { USERPROFILE: "C:\\Users\\tester" },
        fileExists: async () => false,
        platform: "win32",
      });

      expect(resolved).toEqual({
        executable: "C:\\Users\\tester\\.local\\bin\\tomcat.exe",
        found: true,
        source: "process-path",
      });
      expect(commands).toEqual(["where.exe tomcat"]);
    });

    it("falls back to tomcat.exe under USERPROFILE when PATH misses", async () => {
      const installed = path.join("C:\\Users\\tester", ".local", "bin", "tomcat.exe");
      const resolved = await resolveTomcatExecutable({
        commandRunner: async () => {
          throw new Error("not found");
        },
        env: { USERPROFILE: "C:\\Users\\tester" },
        fileExists: async (targetPath) => targetPath === installed,
        platform: "win32",
      });

      expect(resolved).toEqual({ executable: installed, found: true, source: "common-path" });
    });
  });
});
