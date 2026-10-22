import { randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { listTomcatSkills, prepareTomcatSkills, syncTomcatSkills, tomcatSkillsDirectory } from "./skills.js";

const cleanups: Array<() => Promise<void>> = [];
const moduleDir = import.meta.dirname;

afterEach(async () => {
  vi.restoreAllMocks();
  await Promise.all(cleanups.splice(0).map(cleanup => cleanup()));
});

async function source() {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "paperclip-tomcat-skill-source-"));
  cleanups.push(() => fs.rm(directory, { recursive: true, force: true }));
  await fs.mkdir(path.join(directory, "scripts"));
  await fs.writeFile(path.join(directory, "SKILL.md"), "---\nname: wanted\ndescription: Fixture skill\n---\nUse scripts/check.sh.\n");
  await fs.writeFile(path.join(directory, "scripts/check.sh"), "printf fixture");
  return directory;
}

function config(entries: Array<{ key: string; runtimeName: string; source: string }>, desiredSkills: string[]) {
  return { paperclipRuntimeSkills: entries, paperclipSkillSync: { desiredSkills } };
}

async function prepare(value: Record<string, unknown>, runId = randomUUID()) {
  const bundle = await prepareTomcatSkills(value, moduleDir, runId);
  cleanups.push(bundle.cleanup);
  return bundle;
}

describe("Tomcat connection-private skills", () => {
  it("copies only assigned skills, including attachments, as real directories", async () => {
    const directory = await source();
    const bundle = await prepare(config([
      { key: "selected", runtimeName: "wanted", source: directory },
      { key: "other", runtimeName: "unwanted", source: directory },
    ], ["selected"]));
    expect(await fs.readdir(bundle.directory)).toEqual(["wanted"]);
    expect((await fs.lstat(path.join(bundle.directory, "wanted"))).isSymbolicLink()).toBe(false);
    expect(await fs.readFile(path.join(bundle.directory, "wanted/SKILL.md"), "utf8")).toContain("Fixture skill");
    expect(await fs.readFile(path.join(bundle.directory, "wanted/scripts/check.sh"), "utf8")).toBe("printf fixture");
    if (process.platform !== "win32") expect((await fs.stat(bundle.directory)).mode & 0o777).toBe(0o700);
    await bundle.cleanup();
    await expect(fs.stat(bundle.directory)).rejects.toMatchObject({ code: "ENOENT" });
    expect(await fs.readFile(path.join(directory, "scripts/check.sh"), "utf8")).toBe("printf fixture");
  });

  it("keeps nested symlinks as links instead of copying an outside target", async () => {
    const directory = await source();
    const outside = await source();
    const target = path.join(outside, "SKILL.md");
    await fs.symlink(target, path.join(directory, "scripts/outside.md"));
    const bundle = await prepare(config([{ key: "selected", runtimeName: "wanted", source: directory }], ["selected"]));
    const copied = path.join(bundle.directory, "wanted/scripts/outside.md");
    expect((await fs.lstat(copied)).isSymbolicLink()).toBe(true);
    expect(await fs.readlink(copied)).toBe(target);
  });

  it("lists and syncs an ephemeral assignment without installing it", async () => {
    const directory = await source();
    const create = vi.spyOn(fs, "mkdir");
    const ctx = { agentId: "agent", companyId: "company", adapterType: "tomcat_local", config: config([{ key: "selected", runtimeName: "wanted", source: directory }], ["selected"]) };
    const snapshot = await listTomcatSkills(ctx);
    expect(snapshot).toMatchObject({ supported: true, mode: "ephemeral", desiredSkills: ["selected"], entries: [{ key: "selected", state: "configured" }] });
    expect(await syncTomcatSkills(ctx, ["selected"])).toEqual(snapshot);
    expect(create).not.toHaveBeenCalled();
  });

  it("honors an explicitly empty assignment instead of discovering bundled skills", async () => {
    const bundle = await prepare({ paperclipRuntimeSkills: [] });
    expect(await fs.readdir(bundle.directory)).toEqual([]);
  });

  it.each(["../outside", "..\\outside", ".", "..", "/absolute"])("rejects unsafe runtime name %s before creating a bundle", async runtimeName => {
    const directory = await source();
    const create = vi.spyOn(fs, "mkdir");
    await expect(prepareTomcatSkills(config([{ key: "selected", runtimeName, source: directory }], ["selected"]), moduleDir, randomUUID()))
      .rejects.toThrow("Invalid Tomcat skill directory name");
    expect(create).not.toHaveBeenCalled();
  });

  it("rejects colliding runtime names rather than overwriting another selected skill", async () => {
    const directory = await source();
    await expect(prepareTomcatSkills(config([
      { key: "one", runtimeName: "same", source: directory },
      { key: "two", runtimeName: "same", source: directory },
    ], ["one", "two"]), moduleDir, randomUUID())).rejects.toThrow("Duplicate Tomcat skill directory name");
  });

  it("cleans a partially copied bundle when an available source cannot be read", async () => {
    const directory = await source();
    const runId = randomUUID();
    const created = tomcatSkillsDirectory(runId);
    await expect(prepareTomcatSkills(config([
      { key: "one", runtimeName: "copied", source: directory },
      { key: "two", runtimeName: "missing", source: path.join(directory, "missing") },
    ], ["one", "two"]), moduleDir, runId)).rejects.toMatchObject({ code: "ENOENT" });
    await expect(fs.stat(created)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("refuses duplicate owners without overwriting or cleaning their files", async () => {
    const runId = randomUUID();
    const value = config([{ key: "selected", runtimeName: "wanted", source: await source() }], ["selected"]);
    const first = await prepare(value, runId);
    await expect(prepare(value, runId)).rejects.toMatchObject({ code: "EEXIST" });
    expect(await fs.readFile(path.join(first.directory, "wanted/scripts/check.sh"), "utf8")).toBe("printf fixture");
    await first.cleanup();
    const replacement = await prepare(value, runId);
    await first.cleanup();
    expect(await fs.stat(replacement.directory)).toBeTruthy();
  });

  it("rejects run IDs that could escape the private prefix", async () => {
    await expect(prepareTomcatSkills({}, moduleDir, "../escape")).rejects.toThrow("Invalid Tomcat run ID");
  });

  it("gives overlapping runs independent directories and cleanup", async () => {
    const directory = await source();
    const value = config([{ key: "selected", runtimeName: "wanted", source: directory }], ["selected"]);
    const [a, b] = await Promise.all([prepare(value), prepare(value)]);
    expect(a.directory).not.toBe(b.directory);
    await a.cleanup();
    expect(await fs.readFile(path.join(b.directory, "wanted/scripts/check.sh"), "utf8")).toBe("printf fixture");
  });
});
