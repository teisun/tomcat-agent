import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    failZero: true,
    color: true,
    timeout: 180_000,
    ui: "tdd",
  });
  mocha.addFile(path.resolve(__dirname, "image-acceptance.test.js"));
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
