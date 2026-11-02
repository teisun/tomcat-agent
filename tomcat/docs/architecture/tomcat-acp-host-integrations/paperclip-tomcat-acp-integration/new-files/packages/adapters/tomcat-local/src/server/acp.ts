import path from "node:path";
import type { AdapterExecutionContext, AdapterExecutionResult, AdapterUsageCheckpoint } from "@paperclipai/adapter-utils";
import { asString, ensureAbsoluteDirectory, parseObject } from "@paperclipai/adapter-utils/server-utils";
import { readAdapterExecutionTarget } from "@paperclipai/adapter-utils/execution-target";
import type { AcpxEngineExecutorOptions } from "@paperclipai/adapter-utils/acpx-engine/execute";
import { shellQuote } from "@paperclipai/adapter-utils/ssh";
import { shellCommandArgs } from "@paperclipai/adapter-utils/sandbox-shell";
import { prepareTomcatSkills, tomcatSkillsPrefix } from "./skills.js";

const moduleDir = import.meta.dirname;
type Executor = (ctx: AdapterExecutionContext) => Promise<AdapterExecutionResult>;
type Options = Omit<AcpxEngineExecutorOptions, "adapterType" | "moduleDir" | "packageRootDir">;

export function buildTomcatAcpConfig(config: Record<string, unknown>): Record<string, unknown> {
  if (config.engine && config.engine !== "auto" && config.engine !== "acp") {
    throw new Error("Tomcat supports the ACP engine only.");
  }
  if (config.warmHandleIdleMs != null && config.warmHandleIdleMs !== 0) {
    throw new Error("Tomcat uses run-scoped private skills; warmHandleIdleMs must be 0.");
  }
  const command = asString(config.command, "tomcat").trim();
  if (!command) throw new Error("Tomcat command must not be empty.");
  const base = asString(config.agentCommand, "").trim() || `${JSON.stringify(command)} acp`;
  const next: Record<string, unknown> = {
    ...config,
    agent: "tomcat",
    agentCommand: base,
    mode: config.mode ?? "persistent",
    permissionMode: config.permissionMode ?? "approve-all",
    nonInteractivePermissions: config.nonInteractivePermissions ?? "deny",
    warmHandleIdleMs: 0,
  };
  if (!asString(next.model, "").trim() || next.model === "default") delete next.model;
  return next;
}

/** Keep both engine and SDK command identities stable; only the existing run env varies. */
export function withTomcatSkillLaunch(baseCommand: string): string {
  const script = `exec "$@" --skills-dir ${shellQuote(tomcatSkillsPrefix())}"\${PAPERCLIP_RUN_ID:?PAPERCLIP_RUN_ID is required}"`;
  return [...["sh", ...shellCommandArgs(script), "tomcat-acp"].map(shellQuote), baseCommand].join(" ");
}

function usageCheckpoint(checkpoint: AdapterUsageCheckpoint): AdapterUsageCheckpoint {
  return {
    ...checkpoint,
    // Tomcat does not yet report a complete receipt for every internal model call.
    complete: false,
    costUsd: checkpoint.costUsd ?? null,
    costStatus: checkpoint.costUsd == null ? "unpriced" : checkpoint.costStatus ?? "reported",
  };
}

/** Execution, MCP, permissions, cancellation, and session persistence remain in the shared engine. */
export function createTomcatAcpExecutor(options: Options = {}): Executor {
  let executor: Executor | null = null;
  return async ctx => {
    const target = readAdapterExecutionTarget({
      executionTarget: ctx.executionTarget,
      legacyRemoteExecution: ctx.executionTransport?.remoteExecution,
    });
    if (target?.kind === "remote") throw new Error("Tomcat local requires the host and shared service to access the same local workspace.");
    const workspace = parseObject(ctx.context.paperclipWorkspace);
    const cwd = asString(workspace.cwd, "").trim() || asString(ctx.config.cwd, "").trim();
    if (!cwd || !path.isAbsolute(cwd)) throw new Error("Tomcat requires an explicit absolute workspace cwd from the host.");
    await ensureAbsoluteDirectory(cwd, { createIfMissing: false });
    ctx.signal?.throwIfAborted();
    const config = buildTomcatAcpConfig({ ...ctx.config, cwd });
    const skills = await prepareTomcatSkills(ctx.config, moduleDir, ctx.runId);
    try {
      ctx.signal?.throwIfAborted();
      if (!executor) {
        const { createAcpxEngineExecutor } = await import("@paperclipai/adapter-utils/acpx-engine/execute");
        executor = createAcpxEngineExecutor({ ...options, adapterType: "tomcat_local", moduleDir, packageRootDir: path.resolve(moduleDir, "../..") });
      }
      const result = await executor({
        ...ctx,
        config: { ...config, agentCommand: withTomcatSkillLaunch(asString(config.agentCommand, "")) },
        onUsage: ctx.onUsage ? checkpoint => ctx.onUsage!(usageCheckpoint(checkpoint)) : undefined,
      });
      return {
        ...result,
        usageComplete: false,
        costUsd: result.costUsd ?? null,
        costStatus: result.costUsd == null ? "unpriced" : result.costStatus ?? "reported",
      };
    } finally {
      await skills.cleanup();
    }
  };
}

export const execute = createTomcatAcpExecutor();
