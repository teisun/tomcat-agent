import type { ChildProcess, SpawnOptions, SpawnSyncOptions, SpawnSyncReturns } from "node:child_process";
import { createRequire } from "node:module";

/**
 * Launch npm / npx / `node_modules/.bin/*` the same way on every platform.
 *
 * On Windows those are `.cmd` batch files: Node refuses to spawn them without a shell
 * (EINVAL), and `shell: true` would need hand-written quoting for paths with spaces.
 * cross-spawn applies the correct cmd.exe escaping on Windows and is plain spawn elsewhere.
 */
type CrossSpawn = {
  (command: string, args: readonly string[], options?: SpawnOptions): ChildProcess;
  sync(command: string, args: readonly string[], options: SpawnSyncOptions): SpawnSyncReturns<string>;
};

const crossSpawn = createRequire(__filename)("cross-spawn") as CrossSpawn;

export function spawnCommand(command: string, args: readonly string[], options: SpawnOptions = {}): ChildProcess {
  return crossSpawn(command, args, options);
}

export type RunCommandOptions = {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  /** Return stdout instead of inheriting the terminal; stderr is then kept for the error message. */
  capture?: boolean;
};

/** Run to completion; a launch failure, signal or non-zero exit throws. */
export function runCommand(command: string, args: readonly string[], options: RunCommandOptions = {}): string {
  const result = crossSpawn.sync(command, args, {
    cwd: options.cwd,
    env: options.env,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const detail = options.capture && result.stderr ? `: ${result.stderr.trim()}` : "";
    throw new Error(`${command} ${args.join(" ")} exited with ${result.status ?? result.signal}${detail}`);
  }
  return options.capture ? result.stdout : "";
}
