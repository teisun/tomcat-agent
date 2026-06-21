import * as fs from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

import {
  TEST_PATH_ENV,
  TOMCAT_EXECUTABLE_NAME,
} from "../constants";

const execFileAsync = promisify(execFile);

type CommandRunner = (
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv,
) => Promise<string>;

type FileExists = (targetPath: string, platform: NodeJS.Platform) => Promise<boolean>;

export interface ResolveTomcatExecutableOptions {
  commandRunner?: CommandRunner;
  configuredPath?: string;
  env?: NodeJS.ProcessEnv;
  fileExists?: FileExists;
  pathWasConfigured?: boolean;
  platform?: NodeJS.Platform;
  shellPath?: string;
}

export interface ResolvedTomcatExecutable {
  executable: string;
  found: boolean;
  source:
    | "common-path"
    | "config"
    | "default"
    | "process-path"
    | "shell-path"
    | "test-env";
}

async function defaultCommandRunner(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv,
): Promise<string> {
  const result = await execFileAsync(command, args, {
    env,
    encoding: "utf8",
  });
  return String(result.stdout ?? "").trim();
}

async function defaultFileExists(
  targetPath: string,
  platform: NodeJS.Platform,
): Promise<boolean> {
  const mode = platform === "win32" ? fsConstants.F_OK : fsConstants.X_OK;
  try {
    await fs.access(targetPath, mode);
    return true;
  } catch {
    return false;
  }
}

function isDefaultCommand(candidate: string | undefined): boolean {
  return !candidate || candidate.trim().length === 0 || candidate.trim() === TOMCAT_EXECUTABLE_NAME;
}

function unique(values: Array<string | undefined>): string[] {
  return [...new Set(values.filter((value): value is string => !!value && value.trim().length > 0))];
}

async function lookupOnProcessPath(
  commandName: string,
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  runCommand: CommandRunner,
): Promise<string | undefined> {
  try {
    const locator = platform === "win32" ? "where.exe" : "which";
    const output = await runCommand(locator, [commandName], env);
    return output.split(/\r?\n/).map((line) => line.trim()).find(Boolean);
  } catch {
    return undefined;
  }
}

async function lookupOnLoginShellPath(
  commandName: string,
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  runCommand: CommandRunner,
  shellPath?: string,
): Promise<string | undefined> {
  if (platform === "win32") {
    return undefined;
  }

  const shells = unique([
    shellPath,
    env.SHELL,
    "/bin/zsh",
    "/bin/bash",
  ]);

  for (const shell of shells) {
    try {
      const output = await runCommand(
        shell,
        ["-lc", `command -v ${commandName}`],
        env,
      );
      const match = output.split(/\r?\n/).map((line) => line.trim()).find(Boolean);
      if (match) {
        return match;
      }
    } catch {
      // Try the next shell candidate.
    }
  }

  return undefined;
}

function commonInstallPaths(
  platform: NodeJS.Platform,
  env: NodeJS.ProcessEnv,
): string[] {
  const home = env.HOME ?? env.USERPROFILE ?? "";
  if (platform === "win32") {
    return unique([
      home ? path.join(home, ".local", "bin", "tomcat.exe") : undefined,
      home ? path.join(home, "bin", "tomcat.exe") : undefined,
    ]);
  }

  return unique([
    home ? path.join(home, ".local", "bin", TOMCAT_EXECUTABLE_NAME) : undefined,
    home ? path.join(home, "bin", TOMCAT_EXECUTABLE_NAME) : undefined,
    "/opt/homebrew/bin/tomcat",
    "/usr/local/bin/tomcat",
    "/usr/bin/tomcat",
  ]);
}

async function lookupCommonInstallPath(
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  fileExists: FileExists,
): Promise<string | undefined> {
  for (const installPath of commonInstallPaths(platform, env)) {
    if (await fileExists(installPath, platform)) {
      return installPath;
    }
  }
  return undefined;
}

async function commandLooksUsable(
  executable: string,
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  runCommand: CommandRunner,
  fileExists: FileExists,
): Promise<boolean> {
  if (executable.includes(path.sep) || (platform === "win32" && executable.includes("\\"))) {
    return fileExists(executable, platform);
  }

  try {
    await runCommand(executable, ["--version"], env);
    return true;
  } catch {
    return false;
  }
}

export async function resolveTomcatExecutable(
  options: ResolveTomcatExecutableOptions = {},
): Promise<ResolvedTomcatExecutable> {
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const runCommand = options.commandRunner ?? defaultCommandRunner;
  const fileExists = options.fileExists ?? defaultFileExists;

  const testOverride = env[TEST_PATH_ENV]?.trim();
  if (testOverride) {
    return {
      executable: testOverride,
      found: true,
      source: "test-env",
    };
  }

  const configuredPath = options.configuredPath?.trim();
  if (options.pathWasConfigured && configuredPath && !isDefaultCommand(configuredPath)) {
    return {
      executable: configuredPath,
      found: await commandLooksUsable(configuredPath, env, platform, runCommand, fileExists),
      source: "config",
    };
  }

  const processPathHit = await lookupOnProcessPath(
    TOMCAT_EXECUTABLE_NAME,
    env,
    platform,
    runCommand,
  );
  if (processPathHit) {
    return {
      executable: processPathHit,
      found: true,
      source: "process-path",
    };
  }

  const shellPathHit = await lookupOnLoginShellPath(
    TOMCAT_EXECUTABLE_NAME,
    env,
    platform,
    runCommand,
    options.shellPath,
  );
  if (shellPathHit) {
    return {
      executable: shellPathHit,
      found: true,
      source: "shell-path",
    };
  }

  const commonPathHit = await lookupCommonInstallPath(env, platform, fileExists);
  if (commonPathHit) {
    return {
      executable: commonPathHit,
      found: true,
      source: "common-path",
    };
  }

  return {
    executable: configuredPath || TOMCAT_EXECUTABLE_NAME,
    found: false,
    source: "default",
  };
}
