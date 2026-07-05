import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

import {
  createHostE2eFixture,
  resolveVsCodeExecutable,
  seedChatUserSettings,
} from "./e2eHostFixture";

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, "..");
  const extensionTestsPath = path.resolve(
    extensionDevelopmentPath,
    "out/test/suite/index.js",
  );
  const fixture = await createHostE2eFixture();
  const userDataDir = await fs.mkdtemp(path.join(os.tmpdir(), "tdev-"));

  try {
    execFileSync("npm", ["run", "build"], {
      cwd: extensionDevelopmentPath,
      stdio: "inherit",
    });
    await seedChatUserSettings(userDataDir);

    await fs.access(extensionTestsPath);
    await runTests({
      extensionDevelopmentPath,
      extensionTestsEnv: fixture.env,
      extensionTestsPath,
      launchArgs: [
        path.resolve(extensionDevelopmentPath, ".."),
        `--user-data-dir=${userDataDir}`,
      ],
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
  } finally {
    await fixture.cleanup();
    await fs.rm(userDataDir, { force: true, recursive: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
