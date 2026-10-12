import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { spawnSync } from "node:child_process";

import { resolveCargoCommand } from "./resolveCargoCommand";

const extensionRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(extensionRoot, "..");
const tomcatCliRoot = path.resolve(repoRoot, "tomcat");
const targetDir = path.resolve(extensionRoot, "src/serveClient");
const targetFile = path.resolve(targetDir, "wire.d.ts");
const checkOnly = process.argv.includes("--check");

function runPrintSchema(schemaOutDir: string): void {
  const cargoCommand = resolveCargoCommand();
  const result = spawnSync(
    cargoCommand,
    ["run", "--bin", "tomcat", "--", "serve", "--print-schema"],
    {
      cwd: tomcatCliRoot,
      encoding: "utf8",
      env: {
        ...process.env,
        TOMCAT__SERVE__SCHEMA_OUT_DIR: schemaOutDir,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  if (result.error) {
    throw new Error(`failed to spawn cargo: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const stderr = result.stderr || result.stdout || "";
    if (stderr) {
      process.stderr.write(stderr);
    }
    throw new Error("failed to run `cargo run --bin tomcat -- serve --print-schema`");
  }
}

function main(): void {
  const schemaOutDir = fs.mkdtempSync(path.join(os.tmpdir(), "tomcat-serve-schema-"));
  try {
    runPrintSchema(schemaOutDir);
    const sourceFile = path.join(schemaOutDir, "serve.d.ts");
    if (!fs.existsSync(sourceFile)) {
      throw new Error(`generated TypeScript file not found: ${sourceFile}`);
    }

    const generated = fs.readFileSync(sourceFile, "utf8");
    const current = fs.existsSync(targetFile)
      ? fs.readFileSync(targetFile, "utf8")
      : undefined;

    if (checkOnly) {
      if (current !== generated) {
        throw new Error(
          `wire.d.ts is out of date. Run \`npm run gen:wire\` to refresh ${path.relative(extensionRoot, targetFile)}.`,
        );
      }
      process.stdout.write("wire.d.ts is up to date.\n");
      return;
    }

    fs.mkdirSync(targetDir, { recursive: true });
    fs.writeFileSync(targetFile, generated);
    process.stdout.write(
      `Generated ${path.relative(extensionRoot, targetFile)} from ${sourceFile}.\n`,
    );
  } finally {
    fs.rmSync(schemaOutDir, { force: true, recursive: true });
  }
}

try {
  main();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
}
