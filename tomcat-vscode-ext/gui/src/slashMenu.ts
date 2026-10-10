import { t as defaultT, type Translator } from "../../src/shared/i18n";
import type { InstructionCard, SharedSlashCommand } from "../../src/serveClient/wire";

export interface SlashMenuItem extends SharedSlashCommand {
  id: string;
  kind: "command" | "skill" | "terminal";
  source?: string;
  path?: string;
}
export interface SlashMenuSection { id: string; title: string; items: SlashMenuItem[] }
export type SlashMenuRow = { type:"item"; id:string; item:SlashMenuItem; groupId:string } | { type:"more"; id:string; count:number; groupId:string; title:string };
export const COLLAPSED_GROUP_LIMIT = 3;
const sourceOrder = (source: string) => source === ".cursor" ? 0 : source === ".agents" ? 1 : 2;

export function buildSlashMenuSections(commands: readonly SharedSlashCommand[], query = "", catalog: readonly InstructionCard[] = [], leading = true, t: Translator = defaultT): SlashMenuSection[] {
  const matches = (name:string, description:string) => `${name} ${description}`.toLowerCase().includes(query.toLowerCase());
  const sections: SlashMenuSection[] = ["skill", "command"].map((kind) => ({
    id: kind === "skill" ? "skills" : "commands", title:kind === "skill" ? t("term.skills") : t("slash.commands"),
    items: catalog.filter((card) => card.kind === kind && matches(card.name,card.description))
      .sort((a,b) => a.name.localeCompare(b.name) || sourceOrder(a.source)-sourceOrder(b.source) || a.source.localeCompare(b.source))
      .map((card) => ({id:card.id,kind:card.kind,name:card.name,usage:`/${card.name}`,summary:card.description,source:card.source,path:card.path})),
  }));
  if (leading) {
    const terminal = commands.map(command => ({ ...command, summary:
      command.name === "reload" ? t("slash.reloadSummary")
      : command.name === "install" ? t("slash.installSummary")
      : command.name === "uninstall" ? t("slash.uninstallSummary") : command.summary,
    }));
    sections.push({ id: "terminal", title: t("slash.terminal"), items: terminal.filter(c => matches(c.name, c.summary)).map(c => ({ ...c, id: `terminal:${c.name}`, kind: "terminal" })) });
  }
  return sections.filter((section) => section.items.length > 0);
}

export function visibleSlashRows(sections: SlashMenuSection[], expandedGroups: ReadonlySet<string>): SlashMenuRow[] {
  return sections.flatMap((section) => {
    const collapse = section.items.length > COLLAPSED_GROUP_LIMIT + 1 && !expandedGroups.has(section.id);
    const rows: SlashMenuRow[] = (collapse ? section.items.slice(0,COLLAPSED_GROUP_LIMIT) : section.items).map((item) => ({type:"item",id:item.id,item,groupId:section.id}));
    if (collapse) rows.push({type:"more",id:`more:${section.id}`,count:section.items.length-COLLAPSED_GROUP_LIMIT,groupId:section.id,title:section.title});
    return rows;
  });
}

/** Only a whole first token is a local terminal operation. */
export function matchSlashCommand(text: string, names: readonly string[]): boolean {
  const token = text.trim().split(/\s/u, 1)[0];
  return token.startsWith("/") && names.includes(token.slice(1));
}
