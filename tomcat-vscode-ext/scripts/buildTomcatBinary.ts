import * as path from "node:path";
import { spawn } from "node:child_process";
import { stat } from "node:fs/promises";
import { resolveCargoCommand } from "./resolveCargoCommand";

const tomcatRoot = path.resolve(__dirname, "..", "..", "tomcat");
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.join(tomcatRoot, "target");
const binary = path.join(targetDir, "debug", process.platform === "win32" ? "tomcat.exe" : "tomcat");

export async function builtTomcatBinary(): Promise<string> {
  if (!(await stat(binary).then(info => info.isFile()).catch(() => false))) {
    throw new Error(`tomcat binary not found after build: ${binary}`);
  }
  return binary;
}

/** Build before any service owns the executable; cold compilation has no journey timeout. */
export async function buildTomcatBinary(): Promise<string> {
  console.log(`[tomcat build] ${binary}`);
  await new Promise<void>((resolve, reject) => {
    const child = spawn(resolveCargoCommand(), ["build", "--bin", "tomcat"], {
      cwd: tomcatRoot,
      env: { ...process.env, CARGO_TARGET_DIR: targetDir },
      windowsHide: true,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`cargo build failed (${code ?? signal}); see cargo output above`));
    });
  });
  return builtTomcatBinary();
}
