import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { execFileSync } from "node:child_process";

import { createHostE2eFixture } from "./e2eHostFixture";
import { packageVsix, PREBUILT_VSIX_ENV } from "./package-vsix";

function run(command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv): void {
  execFileSync(command, args, {
    cwd,
    env,
    stdio: "inherit",
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
    run("npm", ["run", "gate:fast"], extensionRoot);
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
    run("npm", ["run", "test:integration"], extensionRoot, gateEnv);
    run("npm", ["run", "test:e2e:vscode-install"], extensionRoot, gateEnv);
    run("npm", ["run", "verify:vsix"], extensionRoot, gateEnv);
  } finally {
    await fixture.cleanup();
    await fs.rm(prebuiltVsixRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
