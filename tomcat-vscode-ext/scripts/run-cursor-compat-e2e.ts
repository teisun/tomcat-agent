import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

import {
  createHostE2eFixture,
  resolveCursorExecutable,
  resolveVsCodeCli,
} from "./e2eHostFixture";

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const harnessRoot = path.resolve(extensionRoot, "e2e-harness");
  const harnessTestsPath = path.resolve(harnessRoot, "out/test/index.js");
  const installRoot = await fs.mkdtemp("/tmp/tcur-");
  const extensionsDir = path.join(installRoot, "extensions");
  const userDataDir = path.join(installRoot, "user-data");
  const vsixPath = path.join(installRoot, "tomcat-vscode-ext.vsix");
  const fixture = await createHostE2eFixture();

  try {
    await fs.mkdir(extensionsDir, { recursive: true });
    await fs.mkdir(userDataDir, { recursive: true });

    execFileSync("npm", ["run", "compile"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    execFileSync("npx", ["tsc", "-p", "e2e-harness/tsconfig.json"], {
      cwd: extensionRoot,
      stdio: "inherit",
    });
    execFileSync(
      "npx",
      ["vsce", "package", "--no-dependencies", "--out", vsixPath],
      {
        cwd: extensionRoot,
        stdio: "inherit",
      },
    );

    // 先用 VSCode 官方安装路径把 VSIX 落到一个隔离的 extensions dir，
    // 再让 Cursor 复用这份目录进行兼容运行验证。
    execFileSync(
      resolveVsCodeCli(),
      [
        "--user-data-dir",
        userDataDir,
        "--extensions-dir",
        extensionsDir,
        "--install-extension",
        vsixPath,
        "--force",
      ],
      {
        stdio: "inherit",
      },
    );

    await fs.access(harnessTestsPath);
    await runTests({
      extensionDevelopmentPath: harnessRoot,
      extensionTestsEnv: fixture.env,
      extensionTestsPath: harnessTestsPath,
      launchArgs: [
        path.resolve(extensionRoot, ".."),
        `--extensions-dir=${extensionsDir}`,
        `--user-data-dir=${userDataDir}`,
      ],
      reuseMachineInstall: true,
      vscodeExecutablePath: resolveCursorExecutable(),
    });
  } finally {
    await fixture.cleanup();
    await fs.rm(installRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
