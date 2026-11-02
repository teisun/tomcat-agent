import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { testEnvironment } from "./test.js";

const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map(root => fs.rm(root, { recursive: true, force: true }))); });

async function fixture(options: { version?: string; acp?: boolean; status?: unknown; statusExit?: number } = {}) {
  const cwd = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "paperclip-tomcat-probe-")));
  roots.push(cwd);
  const command = path.join(cwd, "fake-tomcat");
  const log = path.join(cwd, "calls.jsonl");
  await fs.writeFile(command, `#!/usr/bin/env node
const fs = require('node:fs');
const options = ${JSON.stringify(options)};
const args = process.argv.slice(2);
fs.appendFileSync(process.env.PROBE_LOG, JSON.stringify(args) + '\\n');
if (args.join(' ') === '--version') console.log('tomcat ' + (options.version ?? '0.1.80'));
else if (args.join(' ') === 'acp --help') console.log(options.acp === false ? 'old build' : 'Tomcat ACP --skills-dir <DIRECTORY>');
else if (args.join(' ') === 'serve status') {
  console.log(JSON.stringify(options.status ?? { service: 'tomcat', running: true, serverVersion: '0.1.80' }));
  process.exitCode = options.statusExit ?? 0;
} else process.exitCode = 99;
`, { mode: 0o755 });
  return { ctx: { companyId: "company-test", adapterType: "tomcat_local", config: { command, cwd, env: { PROBE_LOG: log } } }, log };
}

// These exercise real subprocess probes on Unix; shared-service Windows acceptance is separate.
describe.skipIf(process.platform === "win32")("Tomcat environment checks", () => {
  it("probes only version, help and status without starting a service or prompting a model", async () => {
    const { ctx, log } = await fixture();
    const result = await testEnvironment(ctx);
    expect(result.status).toBe("pass");
    expect(result.checks.map(check => check.code)).toContain("tomcat_service_ready");
    expect((await fs.readFile(log, "utf8")).trim().split("\n").map(line => JSON.parse(line)))
      .toEqual([["--version"], ["acp", "--help"], ["serve", "status"]]);
  });

  it("reports a missing executable", async () => {
    const { ctx } = await fixture();
    ctx.config.command = path.join(ctx.config.cwd, "missing");
    expect(await testEnvironment(ctx)).toMatchObject({ status: "fail", checks: [{ code: "tomcat_command_missing" }] });
  });

  it("rejects a same-version build without the ACP capability", async () => {
    const { ctx } = await fixture({ acp: false });
    expect(await testEnvironment(ctx)).toMatchObject({ status: "fail", checks: [{ code: "tomcat_acp_unavailable" }] });
  });

  it("accepts an installed CLI with a stopped service without starting it", async () => {
    const { ctx, log } = await fixture({ status: { running: false } });
    expect(await testEnvironment(ctx)).toMatchObject({ status: "pass", checks: [{ code: "tomcat_service_stopped", level: "info" }, { code: "tomcat_probe_scope" }] });
    expect(await fs.readFile(log, "utf8")).not.toContain('"start"');
  });

  it("uses capabilities rather than a minimum CLI version or equal service version", async () => {
    const { ctx, log } = await fixture({ version: "0.1.79", status: { service: "tomcat", running: true, serverVersion: "0.1.81" } });
    const result = await testEnvironment(ctx);
    expect(result.status).toBe("pass");
    expect(result.checks.find(check => check.code === "tomcat_service_ready")?.message).toContain("0.1.81");
    expect((await fs.readFile(log, "utf8")).trim().split("\n").map(line => JSON.parse(line)))
      .toEqual([["--version"], ["acp", "--help"], ["serve", "status"]]);
  });

  it("reports an unreachable service", async () => {
    const { ctx } = await fixture({ statusExit: 1 });
    expect(await testEnvironment(ctx)).toMatchObject({ status: "fail", checks: [{ code: "tomcat_service_unreachable" }] });
  });

  it("does not claim that an arbitrary custom launcher was probed", async () => {
    const { ctx, log } = await fixture();
    expect(await testEnvironment({ ...ctx, config: { ...ctx.config, agentCommand: "custom-launcher acp" } }))
      .toMatchObject({ status: "warn", checks: [{ code: "tomcat_custom_command" }] });
    await expect(fs.stat(log)).rejects.toMatchObject({ code: "ENOENT" });
  });
});
