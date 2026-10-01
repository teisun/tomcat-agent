import { forwardRef, useEffect, useImperativeHandle, useState } from "react";
import type { SlashMenuItem, SlashMenuSection } from "../slashMenu";

export interface SlashCommandMenuHandle { onKeyDown(event: KeyboardEvent): boolean }
interface Props { sections: SlashMenuSection[]; query: string; leading: boolean; open: boolean; onSelect(item: SlashMenuItem): void; onClose(): void }

export const SlashCommandMenu = forwardRef<SlashCommandMenuHandle, Props>(function SlashCommandMenu({sections, query, leading, open, onSelect, onClose}, ref) {
  const items = sections.flatMap((section) => section.items);
  const [selected, setSelected] = useState(0);
  useEffect(() => { setSelected(0); }, [query, open]);
  useEffect(() => { setSelected((index) => Math.min(index, Math.max(0, items.length - 1))); }, [items.length]);
  useImperativeHandle(ref, () => ({
    onKeyDown(event) {
      if (!open || !items.length || event.isComposing) return false;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setSelected((index) => (index + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length);
        return true;
      }
      if (event.key === "Enter" || event.key === "Tab") { event.preventDefault(); onSelect(items[selected]); return true; }
      if (event.key === "Escape" || event.key === "Esc") { event.preventDefault(); onClose(); return true; }
      return false;
    },
  }), [items, selected, open, onSelect, onClose]);
  if (!open || !items.length) return null;
  return <div className="tc-session-dropdown tc-context-search-dropdown" role="listbox" aria-label="斜杠命令" aria-activedescendant={`slash-command-${items[selected]?.name}`} data-testid="slash-command-menu">
    {sections.map((section) => <div key={section.id} role="group" aria-label={section.title} className="tc-session-group">
      <div className="tc-session-group__header">{section.title}</div>
      {section.items.map((item) => <button key={`${item.kind}:${item.name}`} id={`slash-command-${item.name}`} role="option" aria-selected={items[selected]?.name === item.name} className={`tc-session-item tc-context-search-dropdown__item${items[selected]?.name === item.name ? " tc-session-item--active" : ""}`} type="button" data-testid="slash-command-option" title={`${item.usage} — ${item.summary}`} onMouseDown={(event) => event.preventDefault()} onClick={() => onSelect(item)}>
        <span className="tc-session-item__title">{item.usage}</span>
        <span className="tc-context-search-dropdown__description">{item.summary}</span>
        {!leading && <span className="tc-context-search-dropdown__description" style={{flex: "0 0 auto"}}>仅在开头生效</span>}
      </button>)}
    </div>)}
  </div>;
});
