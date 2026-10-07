import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SessionFilesDock } from "./SessionFilesDock";
import type { SessionFileView } from "../../../src/shared/sessionFiles";
const rows: SessionFileView[] = [{ path: "/src/app.ts", status: "modified", restorable: true }, { path: "/logo.png", status: "added", restorable: true }, { path: "/skip", status: "modified", restorable: false }];
afterEach(cleanup);
it("collapsed Undo All cancels without mutation and confirms only eligible exact paths", () => {
  const onIntent = vi.fn(); render(<SessionFilesDock sessionId="s" files={{ sourceTurnId: "u", files: rows }} busy={false} onIntent={onIntent} />);
  fireEvent.click(screen.getByRole("button", { name: "Undo All" })); expect(screen.getByRole("button", { name: "Expand files" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Cancel/ })); expect(onIntent).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Undo All" }));
  expect(screen.getByTestId("undo-files-body").textContent).toContain("1 file cannot be undone");
  fireEvent.click(screen.getByRole("button", { name: /Undo 2 Files/ }));
  expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({ type: "restoreSessionFiles", data: expect.objectContaining({ sessionId: "s", sourceTurnId: "u", paths: ["/src/app.ts", "/logo.png"] }) }));
  expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);
});
it("folding preserves in-flight restoration; wrong-source replies do not settle; errors remain visible collapsed", () => {
  const onIntent = vi.fn(); render(<SessionFilesDock sessionId="s" files={{ sourceTurnId: "u", files: rows }} busy={false} onIntent={onIntent} />);
  fireEvent.click(screen.getByRole("button", { name: "Expand files" }));
  fireEvent.click(screen.getAllByTestId("undo-file")[0]); fireEvent.click(screen.getByRole("button", { name: /Undo File/ }));
  const requestId = onIntent.mock.calls.at(-1)![0].data.requestId;
  fireEvent.click(screen.getByRole("button", { name: "Collapse files" }));
  fireEvent(window, new MessageEvent("message", { data: { channel: "event", content: { type: "restoreSessionFilesResult", sessionId: "s", sourceTurnId: "old", requestId, success: true } } }));
  expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", true);
  fireEvent(window, new MessageEvent("message", { data: { channel: "event", content: { type: "restoreSessionFilesResult", sessionId: "s", sourceTurnId: "u", requestId, success: false, error: "failed" } } }));
  expect(screen.getByRole("alert").textContent).toBe("failed");
  expect(screen.getByTestId("undo-all-files")).toHaveProperty("disabled", false);
});
