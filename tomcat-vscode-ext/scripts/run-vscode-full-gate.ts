import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { createHostE2eFixture } from "./e2eHostFixture";
import { packageVsix, PREBUILT_VSIX_ENV } from "./package-vsix";
import { spawnCommand } from "./runCommand";

/** Keep the bundled fixture's IPC server responsive while a child runs its tests. */
export function runGateCommand(command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawnCommand(command, args, { cwd, env, stdio: "inherit" });
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} ${args.join(" ")} exited with ${code ?? signal}`));
    });
  });
}

function currentVsCodeTarget(): string | undefined {
  if (process.platform === "darwin" && process.arch === "arm64") {
    return "darwin-arm64";
  }
  if (process.platform === "darwin" && process.arch === "x64") {
    return "darwin-x64";
  }
  if (process.platform === "linux" && process.arch === "x64") {
    return "linux-x64";
  }
  if (process.platform === "win32" && process.arch === "x64") {
    return "win32-x64";
  }
  return undefined;
}

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const prebuiltVsixRoot = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-vsix-full-gate-"));
  const fixture = await createHostE2eFixture();

  try {
    await runGateCommand("npm", ["run", "gate:fast"], extensionRoot);
    const prebuiltVsixPath = packageVsix({
      bundleBinaryPath: fixture.fakeServePath,
      extensionRoot,
      outPath: path.join(prebuiltVsixRoot, "tomcat-vscode-ext.vsix"),
      skipBuild: false,
      target: currentVsCodeTarget(),
    });
    const gateEnv: NodeJS.ProcessEnv = {
      ...process.env,
      [PREBUILT_VSIX_ENV]: prebuiltVsixPath,
    };
    await runGateCommand("npm", ["run", "test:integration"], extensionRoot, gateEnv);
    await runGateCommand("npm", ["run", "test:e2e:vscode-install"], extensionRoot, gateEnv);
    await runGateCommand("npm", ["run", "verify:vsix"], extensionRoot, gateEnv);
  } finally {
    await fixture.cleanup();
    await fs.rm(prebuiltVsixRoot, { force: true, recursive: true });
  }
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
