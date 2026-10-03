import { forwardRef, useEffect, useImperativeHandle, useMemo, useState } from "react";
import { COLLAPSED_GROUP_LIMIT, visibleSlashRows, type SlashMenuItem, type SlashMenuSection, type SlashMenuRow } from "../slashMenu";

export interface SlashCommandMenuHandle { onKeyDown(event: KeyboardEvent): boolean }
interface Props { sections: SlashMenuSection[]; query: string; open: boolean; onSelect(item: SlashMenuItem): void; onClose(): void }
const optionId = (id:string) => `slash-command-${encodeURIComponent(id)}`;

export const SlashCommandMenu = forwardRef<SlashCommandMenuHandle, Props>(function SlashCommandMenu({sections,query,open,onSelect,onClose},ref) {
  const [expandedGroups,setExpandedGroups] = useState<Set<string>>(new Set());
  const rows = useMemo(() => visibleSlashRows(sections,expandedGroups),[sections,expandedGroups]);
  const [selectedId,setSelectedId] = useState<string | null>(null);
  const selected = rows.find((row) => row.id === selectedId) ?? rows[0];
  useEffect(() => { setSelectedId(null); },[query,open]);
  useEffect(() => { if (!open) setExpandedGroups(new Set()); },[open]);
  useEffect(() => {
    if (open && selected) document.getElementById(optionId(selected.id))?.scrollIntoView?.({block:"nearest"});
  },[open,selected?.id]);
  const activate = (row:SlashMenuRow) => {
    if (row.type === "item") { onSelect(row.item); return; }
    setExpandedGroups((current) => new Set([...current,row.groupId]));
    setSelectedId(sections.find((section) => section.id === row.groupId)?.items[COLLAPSED_GROUP_LIMIT]?.id ?? null);
  };
  useImperativeHandle(ref,() => ({onKeyDown(event) {
    if (!open || !rows.length || event.isComposing) return false;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const index = rows.findIndex((row) => row.id === selected?.id);
      setSelectedId(rows[(index + (event.key === "ArrowDown" ? 1 : rows.length-1)) % rows.length].id);
      return true;
    }
    if ((event.key === "Enter" || event.key === "Tab") && selected) { event.preventDefault(); activate(selected); return true; }
    if (event.key === "Escape" || event.key === "Esc") { event.preventDefault(); onClose(); return true; }
    return false;
  }}),[rows,selected,open,onSelect,onClose,sections]);
  if (!open || !rows.length) return null;
  return <div className="tc-session-dropdown tc-context-search-dropdown tc-slash-menu" role="listbox" aria-label="斜杠命令" aria-activedescendant={selected ? optionId(selected.id) : undefined} data-testid="slash-command-menu">
    {sections.map((section) => <div key={section.id} role="group" aria-label={section.title} className="tc-session-group">
      <div className="tc-session-group__header">{section.title}</div>
      {rows.filter((row) => row.groupId === section.id).map((row) => <button key={row.id} id={optionId(row.id)} role="option" aria-selected={selected?.id === row.id} aria-label={row.type === "more" ? `Show ${row.count} more ${row.title}` : undefined} className={`tc-session-item tc-slash-menu__item${selected?.id === row.id ? " tc-session-item--active" : ""}`} type="button" data-testid={row.type === "more" ? "slash-command-more" : "slash-command-option"} title={row.type === "item" ? `${row.item.usage}${row.item.summary ? ` — ${row.item.summary}` : ""}${row.item.source ? ` · ${row.item.source}` : ""}` : undefined} onMouseDown={(event) => event.preventDefault()} onClick={() => activate(row)}>
        {row.type === "more" ? <span className="tc-slash-menu__more">Show {row.count} more</span> : <>
          <span className="tc-slash-menu__heading"><span className="tc-session-item__title">{row.item.usage}</span>{row.item.source && <span className="tc-slash-menu__source">{row.item.source}</span>}</span>
          {row.item.summary && <span className="tc-slash-menu__description">{row.item.summary}</span>}
        </>}
      </button>)}
    </div>)}
  </div>;
});
