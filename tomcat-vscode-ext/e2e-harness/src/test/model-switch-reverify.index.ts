import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    color: true,
    timeout: 120000,
    ui: "tdd",
  });

  mocha.addFile(path.resolve(__dirname, "model-switch-reverify.test.js"));

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
