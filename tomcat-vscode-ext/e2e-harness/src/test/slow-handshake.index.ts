import * as path from "node:path";

import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({
    color: true,
    timeout: 60_000,
    ui: "tdd",
  });

  mocha.grep(/connects when serve takes longer than the former handshake budget/);
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
