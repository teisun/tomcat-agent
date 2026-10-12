import { createRef } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SessionFilesList } from "./SessionFilesList";
import type { SessionFileView } from "../../../src/shared/sessionFiles";
const files: SessionFileView[] = [{ path: "/src/app.ts", displayPath: "src/app.ts", status: "modified", added: 12, removed: 3, restorable: true }, { path: "/tests/app.ts", status: "modified", added: 2, removed: 0, restorable: false, blockedReason: "backup_missing" }];
afterEach(cleanup);
it("preserves file formatting and separates diff and undo callbacks", () => {
  const onIntent = vi.fn(), onUndo = vi.fn();
  render(<SessionFilesList sessionId="s" sourceTurnId="u" files={files} busy={false} onIntent={onIntent} onUndo={onUndo} listRef={createRef()} />);
  expect(screen.getByText("+12")).toBeTruthy(); expect(screen.getByText("-3")).toBeTruthy(); expect(screen.getByText("src")).toBeTruthy(); expect(screen.getByText("tests")).toBeTruthy();
  fireEvent.click(screen.getAllByTestId("session-file-diff")[0]);
  expect(onIntent).toHaveBeenCalledWith(expect.objectContaining({ type: "openSessionFileDiff", data: { sessionId: "s", sourceTurnId: "u", path: "/src/app.ts" } }));
  fireEvent.click(screen.getAllByTestId("undo-file")[0]); expect(onUndo).toHaveBeenCalledWith(files[0]);
  expect(screen.queryByTestId("undo-all-files")).toBeNull();
});
it("busy blocks undo, not viewing the original diff", () => {
  const onIntent = vi.fn(), onUndo = vi.fn();
  render(<SessionFilesList sessionId="s" sourceTurnId="u" files={files} busy onIntent={onIntent} onUndo={onUndo} listRef={createRef()} />);
  screen.getAllByTestId("undo-file").forEach(button => expect(button).toHaveProperty("disabled", true));
  fireEvent.click(screen.getAllByTestId("session-file-diff")[0]); expect(onIntent).toHaveBeenCalled(); expect(onUndo).not.toHaveBeenCalled();
});
