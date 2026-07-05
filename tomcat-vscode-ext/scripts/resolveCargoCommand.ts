import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

export function resolveCargoCommand(): string {
  const configured = process.env.CARGO?.trim();
  if (configured) {
    return configured;
  }

  const executable = process.platform === "win32" ? "cargo.exe" : "cargo";
  const homeCandidate = path.join(os.homedir(), ".cargo", "bin", executable);
  if (fs.existsSync(homeCandidate)) {
    return homeCandidate;
  }

  return "cargo";
}
