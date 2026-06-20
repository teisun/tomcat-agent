import * as fs from "node:fs/promises";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

import { runTests } from "@vscode/test-electron";

import {
  createHostE2eFixture,
  resolveVsCodeExecutable,
} from "./e2eHostFixture";

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, "..");
  const extensionTestsPath = path.resolve(
    extensionDevelopmentPath,
    "out/test/suite/index.js",
  );
  const fixture = await createHostE2eFixture();

  try {
    execFileSync("npm", ["run", "compile"], {
      cwd: extensionDevelopmentPath,
      stdio: "inherit",
    });

    await fs.access(extensionTestsPath);
    await runTests({
      extensionDevelopmentPath,
      extensionTestsEnv: fixture.env,
      extensionTestsPath,
      launchArgs: [
        path.resolve(extensionDevelopmentPath, ".."),
        "--disable-extensions",
      ],
      vscodeExecutablePath: resolveVsCodeExecutable(),
    });
  } finally {
    await fixture.cleanup();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
