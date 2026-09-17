import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    color: true,
    failZero: true,
    timeout: 120_000,
    ui: "tdd",
  });
  mocha.addFile(path.resolve(__dirname, "connectors-installed-acceptance.test.js"));
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
