import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const exec = promisify(execFile);
const sourceScripts = fileURLToPath(new URL("../scripts", import.meta.url));

// Exercise the actual runner in an isolated installation. Vite and shot are
// orchestration fixtures: these tests do NOT claim screenshot/UI acceptance.
async function fixture(viteExitsEarly = false) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-gui-smoke-runner-"));
  const scripts = path.join(root, "scripts");
  const viteDir = path.join(root, "gui/node_modules/vite/bin");
  const out = path.join(root, "out");
  await fs.mkdir(scripts, { recursive: true });
  await fs.mkdir(viteDir, { recursive: true });
  await fs.mkdir(out);
  for (const name of ["run-gui-acceptance.mjs", "accept-package-install.mjs", "accept-project-agents-paths.mjs"]) {
    await fs.copyFile(path.join(sourceScripts, name), path.join(scripts, name));
  }
  await fs.writeFile(path.join(viteDir, "vite.js"), viteExitsEarly ? "process.exit(7);\n" : `
const http = require('node:http');
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => res.end('<div id="root">runner fixture</div>')).listen(port, '127.0.0.1');
`);
  const shot = path.join(root, "shot-fixture.mjs");
  await fs.writeFile(shot, "// No browser: runner contract test only.\n");
  return {
    root, out,
    run: (name: string, timeout = 3_000) => exec(process.execPath, [path.join(scripts, name)], {
      cwd: root,
      timeout,
      env: { HOME: root, NO_PROXY: "127.0.0.1,localhost", no_proxy: "127.0.0.1,localhost", TOMCAT_VERIFY_SHOT: shot, TOMCAT_ACCEPT_OUT: out },
    }),
  };
}

describe("standalone GUI smoke evidence", () => {
  it.each(["accept-package-install.mjs", "accept-project-agents-paths.mjs"])("%s does not claim unperformed host simulation", async (script) => {
    const fx = await fixture();
    try {
      const result = await fx.run(script);
      const files = (await fs.readdir(fx.out)).filter(name => name.endsWith(".json"));
      expect(files).toHaveLength(1);
      const evidence = JSON.parse(await fs.readFile(path.join(fx.out, files[0]), "utf8"));
      expect(evidence.scope).toBe("gui-shell-render-smoke");
      expect(evidence.hostReply).toBe("not-injected");
      expect(evidence).not.toHaveProperty("simulatedReplies");
      expect(result.stdout).toContain("not feature acceptance");
    } finally {
      await fs.rm(fx.root, { recursive: true, force: true });
    }
  });

  it("fails on an already-exited Vite instead of reporting success", async () => {
    const fx = await fixture(true);
    try {
      // The old missed-exit promise never settled; Node then exited 0 with no artifacts.
      const error = await fx.run("accept-package-install.mjs", 12_000).then(() => undefined, error => error);
      expect(error).toBeDefined();
      expect(error.killed).toBe(false);
      expect(error.code).toBe(1);
      expect(error.stderr).toContain("Vite");
    } finally {
      await fs.rm(fx.root, { recursive: true, force: true });
    }
  }, 15_000);
});
