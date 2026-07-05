import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    color: true,
    timeout: 60000,
    ui: "tdd",
  });

  mocha.grep(
    /uses the expected executable source when the host fixture asks for it|shows the expected onboarding prompt when requested by the host fixture|recovers from a setup-required startup when the test fixture auto-runs init/,
  );
  mocha.addFile(path.resolve(__dirname, "installed.test.js"));

  await new Promise<void>((resolve, reject) => {
    mocha.run((failures) => {
      if (failures > 0) {
        reject(new Error(`${failures} tests failed.`));
        return;
      }
      resolve();
    });
  });
}
