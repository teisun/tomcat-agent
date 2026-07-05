import fs from "node:fs";
import path from "node:path";

import { parseCargoVersion, validateCliReleaseTag } from "./guards.mjs";

const repoRoot = process.argv[2] ? path.resolve(process.argv[2]) : process.cwd();
const tag = process.argv[3] ?? process.env.GITHUB_REF_NAME;

if (!tag) {
  throw new Error("CLI tag guard requires a tag argument or GITHUB_REF_NAME");
}

const cargoToml = fs.readFileSync(path.join(repoRoot, "tomcat", "Cargo.toml"), "utf8");
const cargoVersion = parseCargoVersion(cargoToml);
validateCliReleaseTag(tag, cargoVersion);
console.log(`CLI tag guard passed: ${tag} == cli-v${cargoVersion}`);
