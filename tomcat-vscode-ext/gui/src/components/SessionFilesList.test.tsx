import { createRef } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionFilesList } from "./SessionFilesList";
import type { SessionFileView } from "../../../src/shared/sessionFiles";
const files:SessionFileView[]=[{path:"/src/app.ts",displayPath:"src/app.ts",status:"modified",added:12,removed:3,restorable:true},{path:"/tests/app.ts",status:"modified",added:2,removed:0,restorable:false,blockedReason:"head_moved"},{path:"/logo.png",status:"added",restorable:true}];
afterEach(cleanup);
describe("SessionFilesList",()=>{
  it("shows counts and distinguishing directories, isolates diff from undo, and cancels without mutation",()=>{
    const onIntent=vi.fn();render(<SessionFilesList sessionId="s" sourceTurnId="u" files={files} busy={false} onIntent={onIntent} listRef={createRef()} />);
    expect(screen.getByText("+12")).toBeTruthy();expect(screen.getByText("-3")).toBeTruthy();
    expect(screen.getByText("src")).toBeTruthy();expect(screen.getByText("tests")).toBeTruthy();
    expect(screen.getAllByTestId("session-file-diff")[0].getAttribute("title")).toBe("src/app.ts");
    fireEvent.click(screen.getAllByTestId("session-file-diff")[0]);
    expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({type:"openSessionFileDiff",data:{sessionId:"s",sourceTurnId:"u",path:"/src/app.ts"}}));
    onIntent.mockClear();fireEvent.click(screen.getAllByTestId("undo-file")[0]);
    expect(screen.getByRole("dialog")).toBeTruthy();expect(onIntent).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:/Cancel/}));expect(onIntent).not.toHaveBeenCalled();
  });
  it("Undo All confirms only eligible files and sends exact turn/path identity",()=>{
    const onIntent=vi.fn();render(<SessionFilesList sessionId="s" sourceTurnId="u4" files={files} busy={false} onIntent={onIntent} listRef={createRef()} />);
    fireEvent.click(screen.getByTestId("undo-all-files"));
    expect(screen.getByTestId("undo-files-body").textContent).toContain("1 file cannot be undone");
    fireEvent.click(screen.getByRole("button",{name:/Undo 2 Files/}));
    expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({type:"restoreSessionFiles",data:expect.objectContaining({sourceTurnId:"u4",paths:["/src/app.ts","/logo.png"]})}));
    expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);
  });
  it("marks busy/unavailable rows disabled without blocking diff",()=>{
    const onIntent=vi.fn();render(<SessionFilesList sessionId="s" sourceTurnId="u" files={files} busy onIntent={onIntent} listRef={createRef()} />);
    expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);screen.getAllByTestId("undo-file").forEach((button)=>expect(button).toHaveProperty("disabled", true));
    fireEvent.click(screen.getAllByTestId("session-file-diff")[0]);expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({type:"openSessionFileDiff"}));
    expect(screen.queryByText("Review")).not.toBeTruthy();
  });
});
