import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionDock, type SessionDockProps } from "./SessionDock";
const todos = [{id:"t",content:"Work",status:"in_progress" as const}];
const files = {sourceTurnId:"u1",files:[{path:"/src/app.ts",status:"modified" as const,added:2,removed:1,restorable:true}]};
function props(extra:Partial<SessionDockProps>={}):SessionDockProps {return {sessionId:"s",busy:false,planTodos:[],sessionTodos:[],onIntent:vi.fn(),...extra};}
afterEach(cleanup);
describe("SessionDock",()=>{
  it("has zero placeholder when empty and a non-tab single source",()=>{
    const view=render(<SessionDock {...props()} />);expect(view.container.innerHTML).toBe("");
    view.rerender(<SessionDock {...props({files})} />);
    expect(screen.queryByRole("tablist")).not.toBeTruthy();
    expect(screen.queryByTestId("session-files-list")).not.toBeTruthy();
    fireEvent.click(screen.getByRole("button",{name:"Expand files"}));
    expect(screen.getByTestId("session-files-list")).toBeTruthy();
    expect(screen.getByTestId("undo-all-files").compareDocumentPosition(screen.getByTestId("session-file-row")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByText("Review")).not.toBeTruthy();
  });
  it("shows a todo-only title, then exposes flat accessible tabs and busy undo",()=>{
    const p=props({busy:true,sessionTodos:todos});const view=render(<SessionDock {...p} />);
    expect(screen.getByTestId("todo-widget-title").textContent).toContain("Work (1/1)");
    view.rerender(<SessionDock {...p} files={files} />);
    const fileTab=screen.getByRole("tab",{name:"Files 1"});fireEvent.click(fileTab);
    expect(fileTab.getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);
    expect(screen.getByTestId("undo-file")).toHaveProperty("disabled", true);
    fireEvent.keyDown(fileTab,{key:"ArrowLeft"});
    expect(screen.getByRole("tab",{name:"Todos 1/1"}).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("todo-widget-list")).toBeTruthy();
    expect(screen.queryByTestId("undo-all-files")).not.toBeTruthy();
  });
  it("keeps files open across question busy/todo updates and resets only list scroll on a new editing turn",()=>{
    const p=props({files});const view=render(<SessionDock {...p} />);
    fireEvent.click(screen.getByRole("button",{name:"Expand files"}));
    const list=screen.getByTestId("session-files-list");list.scrollTop=40;
    view.rerender(<SessionDock {...p} busy sessionTodos={todos} />);
    expect(screen.getByRole("tab",{name:"Files 1"}).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("session-files-list")).toBe(list);expect(list.scrollTop).toBe(40);
    view.rerender(<SessionDock {...p} busy sessionTodos={[{...todos[0],content:"Updated"}]} />);
    expect(screen.getByTestId("session-files-list")).toBe(list);
    view.rerender(<SessionDock {...p} files={{...files,sourceTurnId:"u4"}} />);
    expect(screen.getByTestId("session-files-list")).toBe(list);expect(list.scrollTop).toBe(0);
    view.rerender(<SessionDock {...p} files={{sourceTurnId:"u4",files:[]}} />);
    expect(screen.queryByTestId("session-dock")).not.toBeTruthy();
  });
  it("keeps an automatic Todos-to-Files selection when the next question starts",()=>{
    const p=props({busy:true,sessionTodos:todos,files});const view=render(<SessionDock {...p} />);
    fireEvent.click(screen.getByRole("tab",{name:"Todos 1/1"}));
    view.rerender(<SessionDock {...p} busy={false} />);
    expect(screen.getByTestId("session-files-list")).toBeTruthy();
    view.rerender(<SessionDock {...p} sessionTodos={[{...todos[0],content:"Answer follow-up"}]} />);
    expect(screen.getByRole("tab",{name:"Files 1"}).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("session-files-list")).toBeTruthy();
  });
  it("closes old confirmation on source change and resets on session key",()=>{
    const p=props({files});const view=render(<SessionDock key="s" {...p} />);
    fireEvent.click(screen.getByRole("button",{name:"Expand files"}));fireEvent.click(screen.getByTestId("undo-file"));
    expect(screen.getByRole("dialog")).toBeTruthy();
    view.rerender(<SessionDock key="s" {...p} files={{...files,sourceTurnId:"u4"}} />);
    expect(screen.queryByRole("dialog")).not.toBeTruthy();
    view.rerender(<SessionDock key="s2" {...p} sessionId="s2" />);
    expect(screen.queryByTestId("session-files-list")).not.toBeTruthy();
  });
  it("shows Retry for a failed empty query instead of pretending there are no changes",()=>{
    const onIntent=vi.fn();render(<SessionDock {...props({onIntent,files:{sourceTurnId:null,files:[],error:"failed"}})} />);
    fireEvent.click(screen.getByRole("button",{name:"Retry"}));
    expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({type:"refreshSessionFiles",data:{sessionId:"s"}}));
  });
});
