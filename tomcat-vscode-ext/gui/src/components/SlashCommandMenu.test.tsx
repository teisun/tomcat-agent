import { act, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";
import { SlashCommandMenu, type SlashCommandMenuHandle } from "./SlashCommandMenu";
import { buildSlashMenuSections, visibleSlashRows } from "../slashMenu";

const catalog = Array.from({length:25},(_,n)=>({id:`skill:s${n}`,kind:"skill" as const,name:`skill-${String(n).padStart(2,"0")}`,description:"Skill description",source:"managed",path:`/skills/s${n}/SKILL.md`}));

describe("grouped slash menu",()=>{
  it.each([3,4,25])("folds only groups with more than four items: %s",(count)=>{
    const sections = buildSlashMenuSections([],"",catalog.slice(0,count));
    const rows = visibleSlashRows(sections,new Set());
    expect(rows).toHaveLength(count === 25 ? 4 : count);
    expect(rows.filter((r)=>r.type === "more")).toHaveLength(count === 25 ? 1 : 0);
  });
  it("keyboard expansion retains focus and selects the fourth item without invoking",()=>{
    const ref = createRef<SlashCommandMenuHandle>(); const onSelect=vi.fn();
    const sections = buildSlashMenuSections([],"",catalog);
    const {rerender}=render(<SlashCommandMenu ref={ref} sections={sections} query="" open onSelect={onSelect} onClose={vi.fn()} />);
    for(let n=0;n<3;n++) act(()=>{ref.current?.onKeyDown(new KeyboardEvent("keydown",{key:"ArrowDown"}));});
    expect(screen.getByTestId("slash-command-menu").getAttribute("aria-activedescendant")).toContain("more%3Askills");
    act(()=>{ref.current?.onKeyDown(new KeyboardEvent("keydown",{key:"Tab"}));});
    expect(onSelect).not.toHaveBeenCalled(); expect(screen.getAllByRole("option")).toHaveLength(25);
    expect(screen.getByRole("option",{selected:true}).textContent).toContain("skill-03");
    rerender(<SlashCommandMenu ref={ref} sections={buildSlashMenuSections([],"skill",catalog)} query="skill" open onSelect={onSelect} onClose={vi.fn()} />);
    expect(screen.queryByTestId("slash-command-more")).toBeNull();
  });
  it("keeps same names addressable by distinct IDs and omits absent descriptions",()=>{
    const sections=buildSlashMenuSections([],"",[{id:"command:a",kind:"command",name:"review",source:".cursor",path:"a",description:""},{id:"command:b",kind:"command",name:"review",source:".agents",path:"b",description:"Description"}]);
    const onSelect=vi.fn(); render(<SlashCommandMenu sections={sections} query="" open onSelect={onSelect} onClose={vi.fn()} />);
    const options=screen.getAllByRole("option"); expect(options[0].id).not.toBe(options[1].id);
    expect(options[0].querySelector('.tc-slash-menu__description')).toBeNull();
    fireEvent.click(options[1]); expect(onSelect.mock.calls[0][0].id).toBe("command:b");
  });
});
