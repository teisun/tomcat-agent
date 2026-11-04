import * as path from "node:path";
import Mocha from "mocha";

export async function run(): Promise<void> {
  const mocha = new Mocha({ color: true, failZero: true, timeout: 120_000, ui: "tdd" });
  mocha.addFile(path.resolve(__dirname, process.env.TOMCAT_RELEASE_EMPTY_WINDOW === "1"
    ? "empty-window.test.js" : "release-bundle.test.js"));
  await new Promise<void>((resolve, reject) => {
    mocha.run(failures => failures ? reject(new Error(`${failures} release-bundle tests failed`)) : resolve());
  });
}
