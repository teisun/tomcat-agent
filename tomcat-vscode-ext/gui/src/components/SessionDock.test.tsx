import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionDock, type SessionDockProps } from "./SessionDock";
const todos = [{ id: "t", content: "Work", status: "in_progress" as const }];
const files = { sourceTurnId: "u1", files: [{ path: "/src/app.ts", status: "modified" as const, added: 2, removed: 1, restorable: true }] };
function props(extra: Partial<SessionDockProps> = {}): SessionDockProps { return { sessionId: "s", busy: false, planTodos: [], sessionTodos: [], onIntent: vi.fn(), ...extra }; }
afterEach(cleanup);
describe("SessionDock independent stack", () => {
  it("has no empty placeholder; Undo All is visible even with Files collapsed", () => {
    const view = render(<SessionDock {...props()} />); expect(view.container.innerHTML).toBe("");
    view.rerender(<SessionDock {...props({ files })} />);
    expect(screen.queryByRole("tablist")).toBeNull(); expect(screen.queryByRole("list")).toBeNull();
    expect(screen.getByRole("button", { name: "Undo All" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Expand files" }));
    expect(screen.getByRole("list")).toBeTruthy();
  });
  it("keeps Messages, Files, Todos in order, allows both details open and busy undo disabled", () => {
    render(<SessionDock {...props({ busy: true, sessionTodos: todos, files, messages: <div data-testid="messages-dock">Messages</div> })} />);
    const panels = [...screen.getByTestId("session-dock").children].filter(node => node instanceof HTMLElement && node.hasAttribute("data-testid"));
    expect(panels.map(node => node.getAttribute("data-testid"))).toEqual(["messages-dock", "files-dock", "todos-dock"]);
    fireEvent.click(screen.getByRole("button", { name: "Expand files" }));
    fireEvent.click(screen.getByRole("button", { name: "Expand todos" }));
    expect(screen.getByRole("button", { name: "Collapse files" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Collapse todos" })).toBeTruthy();
    expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);
  });
  it("does not close Files on todo or source updates and only resets scroll on source change", () => {
    const p = props({ files }); const view = render(<SessionDock {...p} />);
    fireEvent.click(screen.getByRole("button", { name: "Expand files" }));
    const list = screen.getByTestId("session-files-list"); list.scrollTop = 40;
    view.rerender(<SessionDock {...p} busy sessionTodos={todos} />);
    expect(screen.getByRole("button", { name: "Collapse files" })).toBeTruthy(); expect(list.scrollTop).toBe(40);
    view.rerender(<SessionDock {...p} busy sessionTodos={[{ ...todos[0], content: "Updated" }]} />);
    expect(screen.getByTestId("session-files-list")).toBe(list);
    view.rerender(<SessionDock {...p} files={{ ...files, sourceTurnId: "u4" }} />); expect(list.scrollTop).toBe(0);
  });
  it("closes stale confirmations on source change and resets expansion on session key", () => {
    const p = props({ files }); const view = render(<SessionDock key="s" {...p} />);
    fireEvent.click(screen.getByRole("button", { name: "Undo All" })); expect(screen.getByRole("dialog")).toBeTruthy();
    view.rerender(<SessionDock key="s" {...p} files={{ ...files, sourceTurnId: "u4" }} />); expect(screen.queryByRole("dialog")).toBeNull();
    view.rerender(<SessionDock key="s2" {...p} sessionId="s2" />); expect(screen.getByRole("button", { name: "Expand files" })).toBeTruthy();
  });
  it("shows Retry for a failed empty query", () => {
    const onIntent = vi.fn(); render(<SessionDock {...props({ onIntent, files: { sourceTurnId: null, files: [], error: "failed" } })} />);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({ type: "refreshSessionFiles", data: { sessionId: "s" } }));
  });
});
