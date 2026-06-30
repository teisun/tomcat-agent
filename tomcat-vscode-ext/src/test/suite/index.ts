import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    ui: "tdd",
    color: true,
    timeout: 60000,
  });

  const grep = process.env.TOMCAT_E2E_GREP;
  if (grep) {
    mocha.grep(new RegExp(grep));
  }

  mocha.addFile(path.resolve(__dirname, "extension.test.js"));
  mocha.addFile(path.resolve(__dirname, "host.e2e.test.js"));

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
