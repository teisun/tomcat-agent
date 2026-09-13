#!/usr/bin/env node

// Browser acceptance for the GUI's project-resource-path presentation.
// This intentionally exercises only the standalone Vite page. The extension-host
// replies described in the evidence file are simulated; filesystem migration and
// serve confirmation are covered by native/serve integration tests instead.

import { spawn } from "node:child_process";
import { access, mkdtemp, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");
const guiRoot = path.join(extensionRoot, "gui");
const verifyShot = process.env.TOMCAT_VERIFY_SHOT
  ?? path.resolve(extensionRoot, "..", "tomcat", "assets", "skills", "verify", "scripts", "shot.mjs");
const outDir = process.env.TOMCAT_ACCEPT_OUT
  ?? await mkdtemp(path.join(os.tmpdir(), "tomcat-accept-project-agents-"));

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, options);
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} ${args.join(" ")} exited with ${signal ?? `code ${code}`}`));
    });
  });
}

async function availablePort() {
  return new Promise((resolve, reject) => {
    const server = http.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close((error) => error ? reject(error) : resolve(address.port));
    });
  });
}

async function waitForPage(url) {
  let lastError;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Vite GUI never became ready at ${url}: ${lastError ?? "unknown error"}`);
}

async function main() {
  await access(verifyShot);
  const port = await availablePort();
  const url = `http://127.0.0.1:${port}`;
  const vite = spawn(process.execPath, [path.join(guiRoot, "node_modules", "vite", "bin", "vite.js"), "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: guiRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let viteError = "";
  vite.stderr.on("data", (chunk) => { viteError += chunk.toString(); });

  try {
    await waitForPage(url);
    for (const [name, viewport] of [["desktop", "1440x900"], ["mobile", "390x844"]]) {
      await run(process.execPath, [
        verifyShot,
        url,
        "--out", outDir,
        "--name", `project-agents-paths-${name}`,
        "--viewport", viewport,
        "--wait", "selector:#root",
      ], { cwd: extensionRoot, stdio: "inherit" });
    }
    await writeFile(path.join(outDir, "project-agents-paths.host-reply-simulation.json"), `${JSON.stringify({
      hostReply: "simulated",
      limitation: "The Vite GUI has no VS Code extension host; this evidence does not prove path migration.",
      simulatedReplies: [
        { type: "settingsState", projectResourceDir: ".agents" },
        { type: "settingsUpdateResult", restartRequired: true },
      ],
      viewports: ["1440x900", "390x844"],
    }, null, 2)}\n`);
    console.log(`[accept-project-agents-paths] wrote PNG/ARIA/console evidence to ${outDir}`);
  } finally {
    vite.kill("SIGTERM");
    await new Promise((resolve) => vite.once("exit", resolve));
    if (vite.exitCode && viteError) console.error(viteError);
  }
}

main().catch((error) => {
  console.error(`[accept-project-agents-paths] ${error.stack ?? error.message}`);
  process.exitCode = 1;
});
