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
      if (!address || typeof address === "string") {
        server.close();
        reject(new Error("failed to allocate a TCP port for Vite"));
        return;
      }
      server.close((error) => error ? reject(error) : resolve(address.port));
    });
  });
}

async function waitForPage(url, child) {
  let lastError;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`Vite exited before becoming ready: ${child.signalCode ?? child.exitCode}`);
    }
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

export async function runGuiRenderSmoke({ scenario }) {
  console.log(`[${scenario}] GUI shell render smoke, not feature acceptance; no host replies are injected.`);
  await access(verifyShot);
  const outputPrefix = `tomcat-smoke-${scenario}-`;
  const outDir = process.env.TOMCAT_ACCEPT_OUT
    ?? await mkdtemp(path.join(os.tmpdir(), outputPrefix));
  const port = await availablePort();
  const url = `http://127.0.0.1:${port}`;
  const vite = spawn(
    process.execPath,
    [
      path.join(guiRoot, "node_modules", "vite", "bin", "vite.js"),
      "--host", "127.0.0.1",
      "--port", String(port),
      "--strictPort",
    ],
    { cwd: guiRoot, stdio: ["ignore", "pipe", "pipe"] },
  );
  // Register before any await: a failed Vite may already be gone when finally runs.
  const viteExited = new Promise((resolve) => vite.once("exit", resolve));
  let viteError = "";
  vite.stderr.on("data", (chunk) => { viteError += chunk.toString(); });

  try {
    await waitForPage(url, vite);
    for (const [name, viewport] of [["desktop", "1440x900"], ["mobile", "390x844"]]) {
      await run(process.execPath, [
        verifyShot,
        url,
        "--out", outDir,
        "--name", `${scenario}-shell-${name}`,
        "--viewport", viewport,
        "--wait", "selector:#root",
      ], { cwd: extensionRoot, stdio: "inherit" });
    }
    await writeFile(path.join(outDir, `${scenario}.gui-shell-smoke.json`), `${JSON.stringify({
      scope: "gui-shell-render-smoke",
      hostReply: "not-injected",
      limitation: "Only the standalone GUI shell was rendered. No feature panel interaction, host reply, installation or path migration was exercised.",
      viewports: ["1440x900", "390x844"],
    }, null, 2)}\n`);
    console.log(`[${scenario}] wrote shell PNG/ARIA/console evidence to ${outDir} (not feature acceptance)`);
  } finally {
    vite.kill("SIGTERM");
    await viteExited;
    if (vite.exitCode && viteError) console.error(viteError);
  }
}
