import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    failZero: true,
    color: true,
    timeout: 120000,
    ui: "tdd",
  });
  const grep = process.env.TOMCAT_E2E_GREP;
  if (grep) {
    mocha.grep(new RegExp(grep));
  }

  mocha.addFile(path.resolve(__dirname, "manual-acceptance.test.js"));

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
