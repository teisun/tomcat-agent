import { describe, expect, it } from "vitest";
import { buildSlashMenuSections, matchSlashCommand } from "./slashMenu";
import { reconcileSessionSnapshot, reconcileStateSnapshot } from "./stateReconcile";
import type { WebviewSessionSnapshot, WebviewStateSnapshot } from "./types";
const commands = ["reload", "install", "uninstall"].map((name) => ({name, usage:`/${name}`, summary:name}));

describe("slash menu and dispatch", () => {
  it("keeps one command section, typed items and the backend order", () => {
    const sections = buildSlashMenuSections(commands);
    expect(sections.map((section) => section.title)).toEqual(["命令"]);
    expect(sections[0].items.map((item) => item.name)).toEqual(commands.map((command) => command.name));
    expect(sections[0].items.every((item) => item.kind === "command")).toBe(true);
    expect(buildSlashMenuSections([])).toEqual([]);
    expect(buildSlashMenuSections(commands, "UN")[0].items.map((item) => item.name)).toEqual(["uninstall"]);
  });
  it.each(["/reload", "  /reload\n", "/install './source with space' agent", "/uninstall package scope"])("routes a leading known token: %s", (input) => {
    expect(matchSlashCommand(input, commands.map((command) => command.name))).toBe(true);
  });
  it.each(["/foo", "/model list", "/Users/me/a.txt", "help /reload", "/reloadx", "/reload/file", "/RELOAD", ""])("keeps ordinary input: %s", (input) => {
    expect(matchSlashCommand(input, commands.map((command) => command.name))).toBe(false);
  });
  it("metadata-only changes survive reconciliation; an unchanged snapshot remains cheap", () => {
    const session:WebviewSessionSnapshot = {sessionId:"s1", agentMode:"chat", busy:false, planTodos:[], sessionTodos:[], timeline:[], pendingAttachments:[], ownedByThisFrontend:true};
    const busy = reconcileSessionSnapshot(session, {...session, commandPending:true});
    expect(busy.commandPending).toBe(true);
    expect(busy).not.toBe(session);
    expect(reconcileSessionSnapshot(busy, {...busy})).toBe(busy);
    const base:WebviewStateSnapshot = {activeSessionId:"s1", availableModels:[], ready:true, modelAdminSupported:false, sessions:[], sessionViews:{s1:session}};
    const changed = reconcileStateSnapshot(base, {...base, slashCommands:commands});
    expect(changed.slashCommands).toEqual(commands);
    expect(changed).not.toBe(base);
    expect(reconcileStateSnapshot(changed, {...changed, slashCommands:commands.map((command) => ({...command}))})).toBe(changed);
  });
});
