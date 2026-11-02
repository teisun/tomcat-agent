import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SessionBar } from "./SessionBar";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { en } from "../../../src/shared/i18n/en";

afterEach(cleanup);
describe("SessionBar settings entry", () => {
  it("labels a host in the existing list and blocks deletion without blocking selection or pinning", () => {
    const choose = vi.fn(); const remove = vi.fn(); const pin = vi.fn();
    const session = { sessionId: "host", title: "Host task", origin: { kind: "acp" as const, clientName: "paperclip" }, updatedAt: Date.now(), busy: false, isCurrent: true, ownedByThisFrontend: false };
    render(<SessionBar activeSessionId="host" sessions={[session]} ready onOpenSettings={vi.fn()} onNewSession={vi.fn()} onSwitchSession={choose} onDeleteSession={remove} onSetPinned={pin} deleteSupported />);
    expect(screen.getByTestId("session-select").textContent).toContain("paperclip");
    fireEvent.click(screen.getByTestId("session-select"));
    const trash = screen.getByRole("button", { name: en["session.delete"] });
    expect(trash).toHaveProperty("disabled", true);
    expect(trash.getAttribute("title")).toBe(en["session.readOnly"]);
    fireEvent.click(trash);
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: en["session.pin"] }));
    expect(pin).toHaveBeenCalledWith("host", true);
    fireEvent.click(screen.getByTestId("session-option"));
    expect(choose).toHaveBeenCalledWith("host");
  });

  it("keeps pinning and deletion open and recovers focus when an action removes its row", () => {
    const pin = vi.fn(); const remove = vi.fn(); const choose = vi.fn();
    const session = { sessionId: "s", title: "A session", updatedAt: Date.now(), busy: false, isCurrent: true, ownedByThisFrontend: true };
    const props = { activeSessionId: "s", ready: true, onOpenSettings: vi.fn(), onNewSession: vi.fn(), onSwitchSession: choose, onSetPinned: pin, onDeleteSession: remove, deleteSupported: true };
    const view = render(<SessionBar {...props} sessions={[session]} />);
    const trigger = screen.getByTestId("session-select");
    fireEvent.click(trigger);
    const row = screen.getByRole("listitem");
    expect(row.querySelectorAll("button")).toHaveLength(3);
    expect(row.querySelector("button button")).toBeNull();
    const pinButton = screen.getByRole("button", { name: en["session.pin"] });
    expect(pinButton.getAttribute("title")).toBe(en["session.pin"]);
    expect(pinButton.querySelector(".codicon-pin")).toBeTruthy();
    pinButton.focus();
    fireEvent.click(pinButton);
    expect(pin).toHaveBeenCalledWith("s", true);
    expect(choose).not.toHaveBeenCalled();
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
    view.rerender(<SessionBar {...props} sessions={[{ ...session, isPinned: true }]} />);
    expect(document.activeElement).toBe(trigger);
    const trash = screen.getByRole("button", { name: en["session.delete"] });
    expect(trash.querySelector(".codicon-trash")).toBeTruthy();
    trash.focus();
    fireEvent.click(trash);
    expect(remove).not.toHaveBeenCalled();
    const cancel = screen.getByTestId("delete-session-cancel");
    fireEvent.blur(window);
    expect(screen.getByTestId("delete-session-dialog")).toBeTruthy();
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
    expect(document.activeElement).toBe(cancel);
    fireEvent.keyDown(cancel, { key: "Escape" });
    expect(remove).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trash);
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
    fireEvent.click(trash);
    fireEvent.mouseDown(screen.getByTestId("delete-session-overlay"));
    expect(screen.queryByTestId("delete-session-dialog")).toBeNull();
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
    fireEvent.click(trash);
    const confirm = screen.getByTestId("delete-session-delete");
    fireEvent.mouseDown(confirm);
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(remove).toHaveBeenCalledExactlyOnceWith("s");
    view.rerender(<SessionBar {...props} sessions={[]} />);
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
    expect(document.activeElement).toBe(trigger);
  });

  it.each(["s0", "s1"])("closes when selecting %s and returns focus without waiting for the host", (sessionId) => {
    const choose = vi.fn();
    const now = Date.now();
    const sessions = ["s0", "s1"].map((id) => ({ sessionId: id, title: `Session ${id}`, updatedAt: now, busy: false, isCurrent: id === "s0", ownedByThisFrontend: true }));
    const props = { sessions, ready: true, onOpenSettings: vi.fn(), onNewSession: vi.fn(), onSwitchSession: choose };
    const view = render(<SessionBar {...props} activeSessionId="s0" />);
    const trigger = screen.getByTestId("session-select");
    const originalLabel = trigger.textContent;
    fireEvent.click(trigger);
    const index = sessions.findIndex(session => session.sessionId === sessionId);
    const option = screen.getAllByTestId("session-option")[index];
    option.focus();
    fireEvent.click(option);
    expect(choose).toHaveBeenCalledExactlyOnceWith(sessionId);
    expect(screen.queryByTestId("session-dropdown")).toBeNull();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(trigger.textContent).toBe(originalLabel);
    expect(document.activeElement).toBe(trigger);
    view.rerender(<SessionBar {...props} activeSessionId={sessionId} />);
    expect(screen.queryByTestId("session-dropdown")).toBeNull();
    fireEvent.click(trigger);
    expect(screen.getAllByTestId("session-option")[index].getAttribute("aria-current")).toBe("true");
  });

  it("keeps unpin, expansion and scrolling inside the open panel", () => {
    const choose = vi.fn(); const pin = vi.fn();
    const sessions = Array.from({ length: 7 }, (_, i) => ({ sessionId: `s${i}`, title: `Session ${i}`, updatedAt: Date.now(), busy: false, isCurrent: i === 0, ownedByThisFrontend: true }));
    const props = { sessions, ready: true, onOpenSettings: vi.fn(), onNewSession: vi.fn(), onSwitchSession: choose, onSetPinned: pin };
    const view = render(<SessionBar {...props} activeSessionId="s0" />);
    fireEvent.click(screen.getByTestId("session-select"));
    expect(screen.getAllByTestId("session-option")).toHaveLength(6);
    fireEvent.click(screen.getByTestId("session-more"));
    fireEvent.scroll(screen.getByTestId("session-dropdown"));
    expect(screen.getAllByTestId("session-option")).toHaveLength(7);
    view.rerender(<SessionBar {...props} sessions={[{ ...sessions[0], isPinned: true }, ...sessions.slice(1)]} activeSessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: en["session.unpin"] }));
    expect(pin).toHaveBeenLastCalledWith("s0", false);
    expect(screen.getByTestId("session-dropdown")).toBeTruthy();
  });

  it.each(["new-session-button", "settings-button", "outside", "escape", "trigger", "window-blur"])("closes for %s", (action) => {
    render(<><SessionBar activeSessionId={null} sessions={[]} ready onOpenSettings={vi.fn()} onNewSession={vi.fn()} onSwitchSession={vi.fn()} /><button data-testid="outside">Outside</button></>);
    const trigger = screen.getByTestId("session-select");
    fireEvent.click(trigger);
    if (action === "escape") {
      fireEvent.keyDown(screen.getByTestId("session-dropdown"), { key: "Escape" });
      expect(document.activeElement).toBe(trigger);
    } else if (action === "window-blur") {
      const outside = screen.getByTestId("outside");
      outside.focus();
      fireEvent.blur(window);
      expect(document.activeElement).toBe(outside);
      fireEvent.focus(window);
    } else {
      const target = action === "trigger" ? trigger : screen.getByTestId(action);
      fireEvent.mouseDown(target);
      fireEvent.click(target);
    }
    expect(screen.queryByTestId("session-dropdown")).toBeNull();
  });

  it("also closes when an external command starts creating a session", () => {
    const props = { activeSessionId: null, sessions: [], ready: true, onOpenSettings: vi.fn(), onNewSession: vi.fn(), onSwitchSession: vi.fn() };
    const view = render(<SessionBar {...props} />);
    fireEvent.click(screen.getByTestId("session-select"));
    view.rerender(<SessionBar {...props} creating />);
    expect(screen.queryByTestId("session-dropdown")).toBeNull();
    expect((screen.getByTestId("session-select") as HTMLButtonElement).disabled).toBe(true);
  });
  it("offers unpin but explains why a busy session cannot be deleted", () => {
    const session = { sessionId: "s", title: "A", updatedAt: 1, busy: true, isPinned: true, isCurrent: true, ownedByThisFrontend: true };
    const view = render(<LocaleProvider locale="en"><SessionBar activeSessionId="s" sessions={[session]} ready onOpenSettings={vi.fn()} onNewSession={vi.fn()} onSwitchSession={vi.fn()} onSetPinned={vi.fn()} onDeleteSession={vi.fn()} deleteSupported /></LocaleProvider>);
    fireEvent.click(screen.getByTestId("session-select"));
    const pin = screen.getByRole("button", { name: en["session.unpin"] });
    expect(pin.querySelector(".codicon-pinned")).toBeTruthy();
    const trash = screen.getByRole("button", { name: en["session.delete"] }) as HTMLButtonElement;
    expect(trash.disabled).toBe(true);
    expect(trash.title).toBe(en["session.delete.busy"]);
    view.rerender(<LocaleProvider locale="zh-CN"><SessionBar activeSessionId="s" sessions={[session]} ready onOpenSettings={vi.fn()} onNewSession={vi.fn()} onSwitchSession={vi.fn()} onSetPinned={vi.fn()} onDeleteSession={vi.fn()} deleteSupported /></LocaleProvider>);
    expect(trash.isConnected).toBe(true);
    expect(trash.disabled).toBe(true);
    expect(pin.isConnected).toBe(true);
    expect(document.documentElement.lang).toBe("zh-CN");
  });
  it.each([{ ready: false, creating: false }, { ready: true, creating: true }])("keeps settings available independently of session readiness: %j", (props) => {
    const open = vi.fn();
    render(<LocaleProvider locale="en"><SessionBar {...props} activeSessionId={null} sessions={[]} onOpenSettings={open} onNewSession={vi.fn()} onSwitchSession={vi.fn()} /></LocaleProvider>);
    const button = screen.getByRole("button", { name: en["settings.title"] }) as HTMLButtonElement;
    expect(button.disabled).toBe(false);
    expect(button.title).toBe(en["settings.title"]);
    expect(button.querySelector(".codicon-settings-gear")).toBeTruthy();
    expect(screen.queryByTestId("compact-context-button")).toBeNull();
    fireEvent.click(button);
    expect(open).toHaveBeenCalledTimes(1);
  });
});
