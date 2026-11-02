import { randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, expect, it } from "vitest";
import type { AdapterExecutionContext } from "@paperclipai/adapter-utils";
import { createTomcatAcpExecutor } from "./acp.js";
import { tomcatSkillsDirectory } from "./skills.js";

const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map(root => fs.rm(root, { recursive: true, force: true }))); });

it.skipIf(process.platform === "win32")("resumes through the real SDK with fresh private skills/MCP, while policy changes still create a new session", async () => {
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "paperclip-tomcat-sdk-")));
  roots.push(root);
  const source = path.join(root, "source");
  await fs.mkdir(source);
  const skillFile = path.join(source, "SKILL.md");
  await fs.writeFile(skillFile, "---\nname: work\ndescription: Test skill\n---\nFirst contents.\n");
  // Exercise the real command tokenizer with spaces, quotes and a literal dollar.
  const agent = path.join(root, "agent 'quote' $ name.mjs");
  await fs.copyFile(fileURLToPath(new URL("./fixtures/resume-agent.mjs", import.meta.url)), agent);
  const trace = path.join(root, "trace.jsonl");
  const commands: string[] = [];
  let connection = 1;
  const ctx: AdapterExecutionContext = {
    runId: randomUUID(), agent: { id: "agent-sdk", companyId: "company-sdk", name: "Tomcat", adapterType: "tomcat_local", adapterConfig: {} },
    runtime: { sessionId: null, sessionParams: null, sessionDisplayId: null, taskKey: "TASK-SDK" },
    context: { paperclipWorkspace: { cwd: root, source: "project_workspace", workspaceId: "workspace-sdk" } },
    config: {
      agentCommand: `${JSON.stringify(process.execPath)} ${JSON.stringify(agent)}`,
      stateDir: path.join(root, "state"), model: "default", promptTemplate: "Do the task.",
      env: { HOME: path.join(root, "home"), TOMCAT_TEST_TRACE: trace },
      paperclipRuntimeSkills: [{ key: "work", runtimeName: "work", source }], paperclipSkillSync: { desiredSkills: ["work"] },
    },
    runtimeMcp: { getServers: () => [{ name: "host-mcp", url: "http://example.test/mcp", token: `fixture-token-${connection}`, connectionId: `connection-${connection}` }] },
    onLog: async () => {}, onMeta: async meta => { commands.push(meta.command); },
  };
  const execute = createTomcatAcpExecutor();
  const first = await execute(ctx);
  expect(first.exitCode, first.errorMessage ?? undefined).toBe(0);
  expect(first.sessionId).toBeTruthy();
  expect(first.sessionParams?.configFingerprint).toEqual(expect.any(String));
  await expect(fs.stat(tomcatSkillsDirectory(ctx.runId))).rejects.toMatchObject({ code: "ENOENT" });

  await fs.writeFile(skillFile, "---\nname: work\ndescription: Test skill\n---\nSecond contents.\n");
  connection = 2;
  const secondCtx = { ...ctx, runId: randomUUID(), runtime: { ...ctx.runtime, sessionParams: first.sessionParams ?? null } };
  const second = await execute(secondCtx);
  expect(second.exitCode, second.errorMessage ?? undefined).toBe(0);
  expect(second.sessionId).toBe(first.sessionId);
  expect(second.sessionParams?.configFingerprint).toBe(first.sessionParams?.configFingerprint);
  expect(commands[1]).toBe(commands[0]);
  await expect(fs.stat(tomcatSkillsDirectory(secondCtx.runId))).rejects.toMatchObject({ code: "ENOENT" });

  const events = (await fs.readFile(trace, "utf8")).trim().split("\n").map(line => JSON.parse(line));
  expect(events.filter(event => event.method === "session/new")).toHaveLength(1);
  const loads = events.filter(event => event.method === "session/load");
  expect(loads).toHaveLength(1);
  expect(loads[0].mcpServers[0].headers).toEqual([{ name: "Authorization", value: "Bearer fixture-token-2" }]);
  const reads = events.filter(event => event.method === "skill-read");
  expect(reads[0].body).toContain("First contents.");
  expect(reads[1].body).toContain("Second contents.");
  expect(reads[1].directory).not.toBe(reads[0].directory);

  const third = await execute({ ...secondCtx, runId: randomUUID(), config: { ...ctx.config, permissionMode: "deny-all" }, runtime: { ...ctx.runtime, sessionParams: second.sessionParams ?? null } });
  expect(third.exitCode, third.errorMessage ?? undefined).toBe(0);
  expect(third.sessionParams?.configFingerprint).not.toBe(second.sessionParams?.configFingerprint);
  const finalEvents = (await fs.readFile(trace, "utf8")).trim().split("\n").map(line => JSON.parse(line));
  expect(finalEvents.filter(event => event.method === "session/new")).toHaveLength(2);
}, 20_000);
