import type { AdapterSkillContext, AdapterSkillSnapshot } from "@paperclipai/adapter-utils";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import {
  buildRuntimeMountedSkillSnapshot,
  isPaperclipSkillSourceMissing,
  readPaperclipRuntimeSkillEntries,
  resolveLegacyPaperclipDesiredSkillNames,
} from "@paperclipai/adapter-utils/server-utils";

export async function listTomcatSkills(ctx: AdapterSkillContext): Promise<AdapterSkillSnapshot> {
  const availableEntries = await readPaperclipRuntimeSkillEntries(ctx.config, import.meta.dirname);
  return buildRuntimeMountedSkillSnapshot({
    adapterType: "tomcat_local",
    availableEntries,
    desiredSkills: resolveLegacyPaperclipDesiredSkillNames(ctx.config, availableEntries),
    configuredDetail: "Will be copied into a private directory for the next Tomcat run.",
  });
}

export async function syncTomcatSkills(ctx: AdapterSkillContext, _desiredSkills: string[]): Promise<AdapterSkillSnapshot> {
  return listTomcatSkills(ctx);
}

export function tomcatSkillsPrefix(): string {
  return path.join(os.tmpdir(), "paperclip-tomcat-skills-");
}

export function tomcatSkillsDirectory(runId: string): string {
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(runId)) throw new Error("Invalid Tomcat run ID");
  return `${tomcatSkillsPrefix()}${runId}`;
}

/** A connection-private bundle: never install run-specific skills into Tomcat's home. */
export async function prepareTomcatSkills(
  config: Record<string, unknown>,
  moduleDir: string,
  runId: string,
): Promise<{ directory: string; cleanup: () => Promise<void> }> {
  const directory = tomcatSkillsDirectory(runId);
  const available = await readPaperclipRuntimeSkillEntries(config, moduleDir);
  const desired = new Set(resolveLegacyPaperclipDesiredSkillNames(config, available));
  const selected = available.filter(entry => desired.has(entry.key) && !isPaperclipSkillSourceMissing(entry));
  const names = new Set<string>();
  for (const entry of selected) {
    // Names are one directory segment on both supported path syntaxes.
    if (!entry.runtimeName || entry.runtimeName === "." || entry.runtimeName === ".." || /[\\/\0]/.test(entry.runtimeName)) {
      throw new Error(`Invalid Tomcat skill directory name: ${entry.runtimeName}`);
    }
    if (names.has(entry.runtimeName)) throw new Error(`Duplicate Tomcat skill directory name: ${entry.runtimeName}`);
    names.add(entry.runtimeName);
  }

  // Exclusive creation: duplicate runs or crash leftovers must not overwrite an owner.
  await fs.mkdir(directory, { mode: 0o700 });
  let cleanupResult: Promise<void> | undefined;
  const cleanup = () => cleanupResult ??= fs.rm(directory, { recursive: true, force: true });
  try {
    for (const entry of selected) {
      // Resolve the explicitly selected root, but preserve nested links. Copying
      // their targets could bypass Tomcat's attachment path-boundary checks.
      await fs.cp(await fs.realpath(entry.source), path.join(directory, entry.runtimeName), {
        recursive: true,
        dereference: false,
        verbatimSymlinks: true,
        force: false,
        errorOnExist: true,
      });
    }
    return { directory, cleanup };
  } catch (error) {
    try {
      await cleanup();
    } catch (cleanupError) {
      throw new AggregateError([error, cleanupError], "Tomcat skill staging and cleanup failed");
    }
    throw error;
  }
}
