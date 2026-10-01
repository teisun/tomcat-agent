import type { SharedSlashCommand } from "../../src/serveClient/wire";

export interface SlashMenuItem extends SharedSlashCommand { kind: "command" }
export interface SlashMenuSection { id: string; title: string; items: SlashMenuItem[] }

export function buildSlashMenuSections(commands: readonly SharedSlashCommand[], query = ""): SlashMenuSection[] {
  const items = commands.filter((command) => command.name.toLowerCase().startsWith(query.toLowerCase())).map((command): SlashMenuItem => ({ ...command, kind: "command" }));
  return items.length ? [{ id: "commands", title: "命令", items }] : [];
}

/** Only a whole first token is a command. Unknown commands, paths and mid-text remain chat. */
export function matchSlashCommand(text: string, names: readonly string[]): boolean {
  const token = text.trim().split(/\s/u, 1)[0];
  return token.startsWith("/") && names.includes(token.slice(1));
}
