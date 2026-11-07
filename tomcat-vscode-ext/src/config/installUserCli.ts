import * as fs from "node:fs/promises";
import { execFile, type ExecFileOptionsWithStringEncoding } from "node:child_process";
import { promisify } from "node:util";

type InstallRunner = (
  executable: string,
  args: string[],
  options: ExecFileOptionsWithStringEncoding,
) => Promise<{ stdout: string; stderr: string }>;

const execFileAsync = promisify(execFile);

/** The bundled program is installation material, never a runtime fallback. */
export async function installUserCli(
  executable: string,
  log: (message: string) => void,
  run: InstallRunner = execFileAsync,
): Promise<void> {
  const at = new Date().toISOString();
  try {
    let file;
    try {
      file = await fs.stat(executable);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      log(`${at} No bundled CLI at ${executable}; automatic installation skipped.`);
      return;
    }
    if (!file.isFile()) throw new Error(`Bundled CLI is not a file: ${executable}`);
    log(`${at} ${executable} self-install --add-to-path`);
    const result = await run(executable, ["self-install", "--add-to-path"], {
      encoding: "utf8", windowsHide: true, timeout: 30_000,
    });
    log(result.stdout);
    log(result.stderr);
    log(`${new Date().toISOString()} self-install exited with code 0`);
  } catch (error) {
    const failure = error as Error & {
      code?: string | number; signal?: string; killed?: boolean;
      stdout?: string; stderr?: string;
    };
    log(failure.stdout ?? "");
    log(failure.stderr ?? "");
    log(`${new Date().toISOString()} CLI installation failed: ${failure.message ?? String(error)} (code=${failure.code ?? "unknown"}, signal=${failure.signal ?? "none"}, killed=${failure.killed ?? false})`);
  }
}
