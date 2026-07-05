import fs from "node:fs";
import path from "node:path";

import { readExtensionVersions, validateExtensionReleaseTag } from "./guards.mjs";

const repoRoot = process.argv[2] ? path.resolve(process.argv[2]) : process.cwd();
const tag = process.argv[3] ?? process.env.GITHUB_REF_NAME;

if (!tag) {
  throw new Error("Extension tag guard requires a tag argument or GITHUB_REF_NAME");
}

const versions = readExtensionVersions(repoRoot);
validateExtensionReleaseTag(tag, versions);

if (process.env.GITHUB_OUTPUT) {
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `bundled_cli_version=${versions.bundledCliVersion}\n`);
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `extension_version=${versions.extensionVersion}\n`);
}

console.log(
  `Extension tag guard passed: ${tag} == ext-v${versions.extensionVersion} (bundled CLI ${versions.bundledCliVersion})`,
);
