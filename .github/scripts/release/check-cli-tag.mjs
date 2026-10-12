import path from "node:path";

import { readRepositoryVersions, validateCliReleaseTag } from "./guards.mjs";

const repoRoot = process.argv[2] ? path.resolve(process.argv[2]) : process.cwd();
const tag = process.argv[3] ?? process.env.GITHUB_REF_NAME;

if (!tag) {
  throw new Error("CLI tag guard requires a tag argument or GITHUB_REF_NAME");
}

const versions = readRepositoryVersions(repoRoot);
validateCliReleaseTag(tag, versions.cliVersion);
console.log(`CLI tag guard passed: ${tag} == cli-v${versions.cliVersion}`);
