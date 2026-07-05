import { validateBundledCliAssets } from "./guards.mjs";

const cliVersion = process.argv[2] ?? process.env.PINNED_CLI_VERSION;
if (!cliVersion) {
  throw new Error("Bundled CLI asset guard requires a CLI version argument or PINNED_CLI_VERSION");
}

const stdin = await new Promise((resolve, reject) => {
  let data = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    data += chunk;
  });
  process.stdin.on("end", () => resolve(data));
  process.stdin.on("error", reject);
});

const assetNames = String(stdin)
  .split(/\r?\n/)
  .map((value) => value.trim())
  .filter(Boolean);

validateBundledCliAssets(cliVersion, assetNames);
console.log(`Bundled CLI assets validated for cli-v${cliVersion}`);
