import { randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AdapterExecutionContext, AdapterUsageCheckpoint } from "@paperclipai/adapter-utils";
import { buildTomcatAcpConfig, createTomcatAcpExecutor } from "./acp.js";
import { tomcatSkillsDirectory } from "./skills.js";

const roots: string[] = [];
afterEach(async () => {
  await Promise.all(roots.splice(0).map(root => fs.rm(root, { recursive: true, force: true })));
});

async function fixture(): Promise<AdapterExecutionContext> {
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "paperclip-tomcat-execute-")));
  roots.push(root);
  await fs.mkdir(path.join(root, "source"));
  await fs.writeFile(path.join(root, "source/SKILL.md"), "---\nname: work\ndescription: Test work skill\n---\nCheck the task.\n");
  return {
    runId: randomUUID(), agent: { id: "agent-tomcat", companyId: "company-test", name: "Tomcat", adapterType: "tomcat_local", adapterConfig: {} },
    runtime: { sessionId: null, sessionParams: null, sessionDisplayId: null, taskKey: "TASK-1" },
    config: {
      command: "fake tomcat", model: "default", stateDir: path.join(root, "state"), env: { HOME: path.join(root, "home"), HOST_MARKER: "host-a" },
      promptTemplate: "Do this task.",
      paperclipRuntimeSkills: [{ key: "work", runtimeName: "work", source: path.join(root, "source") }],
      paperclipSkillSync: { desiredSkills: ["work"] },
    },
    context: { issueId: "issue-test", paperclipWorkspace: { cwd: root, source: "project_workspace", workspaceId: "workspace-test" } },
    onLog: async () => {},
  };
}

type Terminal = { status: "completed" | "failed" | "cancelled"; stopReason: string };
function fakeRuntime(hold = false, failed = false) {
  let finish!: (terminal: Terminal) => void;
  let markStarted!: () => void;
  const started = new Promise<void>(resolve => { markStarted = resolve; });
  const result = hold ? new Promise<Terminal>(resolve => { finish = resolve; })
    : Promise.resolve<Terminal>({ status: failed ? "failed" : "completed", stopReason: failed ? "error" : "end_turn" });
  const cancel = vi.fn(async () => { finish({ status: "cancelled", stopReason: "cancelled" }); });
  const runtime = {
    ensureSession: vi.fn(async (input: Record<string, unknown>) => ({
      sessionKey: input.sessionKey, backend: "acpx", runtimeSessionName: "runtime-tomcat", cwd: input.cwd,
      acpxRecordId: "record-tomcat", backendSessionId: "session-tomcat", agentSessionId: "session-tomcat",
    })),
    startTurn: vi.fn(() => {
      markStarted();
      return { requestId: "request-tomcat", result, cancel, closeStream: async () => {}, events: {
        async *[Symbol.asyncIterator]() {
          yield { type: "text_delta", text: "Task output", stream: "output", tag: "agent_message_chunk" };
          await result;
        },
      } };
    }),
    getCapabilities: () => ({ controls: [] }), getStatus: async () => ({}),
    setConfigOption: async () => {}, setMode: async () => {}, cancel: async () => {},
    close: vi.fn(async () => {}),
  };
  return { runtime, started, cancel };
}

function inspectSkills(ctx: AdapterExecutionContext) {
  const directory = tomcatSkillsDirectory(ctx.runId);
  ctx.onMeta = async meta => {
    expect(meta.command).not.toContain(ctx.runId);
    expect(await fs.readFile(path.join(directory, "work/SKILL.md"), "utf8")).toContain("Check the task.");
  };
  return () => directory;
}

describe("Tomcat ACP adapter", () => {
  it("maps only the missing launch settings, preserving model, environment and permission overrides", () => {
    expect(buildTomcatAcpConfig({ model: "default" })).toEqual({ agent: "tomcat", agentCommand: '"tomcat" acp', mode: "persistent", permissionMode: "approve-all", nonInteractivePermissions: "deny", warmHandleIdleMs: 0 });
    const env = { HOST_MARKER: "x" };
    expect(buildTomcatAcpConfig({ command: "/path with spaces/tomcat", model: "chosen", permissionMode: "deny-all", mode: "oneshot", env }))
      .toMatchObject({ agentCommand: '"/path with spaces/tomcat" acp', model: "chosen", permissionMode: "deny-all", mode: "oneshot", env });
    expect(buildTomcatAcpConfig({ agentCommand: "custom-launcher acp" }).agentCommand).toBe("custom-launcher acp");
    expect(() => buildTomcatAcpConfig({ engine: "cli" })).toThrow("ACP engine only");
    expect(() => buildTomcatAcpConfig({ warmHandleIdleMs: 1 })).toThrow("warmHandleIdleMs must be 0");
  });

  it("uses the host workspace and shared runtime, and does not invent a complete or zero-cost receipt", async () => {
    const ctx = await fixture();
    ctx.config.cwd = "/ignored-config-cwd";
    const checkpoints: AdapterUsageCheckpoint[] = [];
    ctx.onUsage = async checkpoint => { checkpoints.push(checkpoint); };
    ctx.runtimeMcp = { getServers: () => [{ name: "host-mcp", url: "http://example.test/mcp", token: "fixture-token", connectionId: "connection-test" }] };
    const directory = inspectSkills(ctx);
    const { runtime } = fakeRuntime();
    runtime.close.mockImplementation(async () => { await fs.stat(directory()); });
    let mcpServers: unknown;
    const execute = createTomcatAcpExecutor({ createRuntime: options => { mcpServers = options.mcpServers; return runtime as never; } });
    const result = await execute(ctx);
    expect(runtime.ensureSession.mock.calls[0]?.[0]).toMatchObject({ agent: "tomcat", cwd: (ctx.context.paperclipWorkspace as { cwd: string }).cwd });
    expect(mcpServers).toEqual([{ type: "http", name: "host-mcp", url: "http://example.test/mcp", headers: [{ name: "Authorization", value: "Bearer fixture-token" }] }]);
    const sessionOptions = runtime.ensureSession.mock.calls[0]![0].sessionOptions as { env: Record<string, string> };
    expect(sessionOptions.env.HOST_MARKER).toBe("host-a");
    expect(runtime.close).toHaveBeenCalled();
    expect(result).toMatchObject({ exitCode: 0, usageComplete: false, costUsd: null, costStatus: "unpriced" });
    expect(checkpoints.at(-1)).toMatchObject({ complete: false, costUsd: null, costStatus: "unpriced" });
    await expect(fs.stat(directory())).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("cleans private skills after a provider failure", async () => {
    const ctx = await fixture();
    const directory = inspectSkills(ctx);
    const { runtime } = fakeRuntime(false, true);
    const result = await createTomcatAcpExecutor({ createRuntime: () => runtime as never })(ctx);
    expect(result.exitCode).not.toBe(0);
    await expect(fs.stat(directory())).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("lets the existing engine cooperatively cancel before cleaning its skills", async () => {
    const ctx = await fixture();
    const directory = inspectSkills(ctx);
    const abort = new AbortController();
    ctx.signal = abort.signal;
    const { runtime, started, cancel } = fakeRuntime(true);
    runtime.close.mockImplementation(async () => { await fs.stat(directory()); });
    const running = createTomcatAcpExecutor({ createRuntime: () => runtime as never })(ctx);
    await started;
    abort.abort();
    const result = await running;
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(result.exitCode).not.toBe(0);
    expect(runtime.close).toHaveBeenCalled();
    await expect(fs.stat(directory())).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("does not fall back to the host process cwd or launch a remote target", async () => {
    const ctx = await fixture();
    const factory = vi.fn();
    const execute = createTomcatAcpExecutor({ createRuntime: factory });
    ctx.context = {};
    await expect(execute(ctx)).rejects.toThrow("explicit absolute workspace cwd");
    ctx.config.cwd = "relative";
    await expect(execute(ctx)).rejects.toThrow("explicit absolute workspace cwd");
    ctx.executionTarget = { kind: "remote", transport: "sandbox", providerKey: "test", remoteCwd: "/work" };
    await expect(execute(ctx)).rejects.toThrow("same local workspace");
    expect(factory).not.toHaveBeenCalled();
  });
});
