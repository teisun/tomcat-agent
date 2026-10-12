import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { MessageQueueDock, QueuedMessageEditor } from "./MessageQueueDock";
import { ComposerSurface, type ComposerSurfaceProps } from "./ComposerSurface";
import type { WebviewMessageQueue } from "../../../src/ui/webview/protocol";

afterEach(cleanup);
const callbacks = () => vi.fn();
function composerProps(): ComposerSurfaceProps {
  return { availableModels: ["A"], busy: true, canInterrupt: true, canPrompt: true, contextLabel: "", modeValue: "chat", modelValue: "A", thinkingLevelValue: "", attachments: [],
    contextSearchLoading: false, contextSearchMatches: [], contextSearchQuery: "", contextSearchTruncated: false,
    onContextSearchClose: callbacks(), onContextSearchOpen: callbacks(), onContextSearchQueryChange: callbacks(), onPickContext: callbacks(), onContextWindowChange: callbacks(),
    onDraftChange: callbacks(), onModeChange: callbacks(), onModelChange: callbacks(), onResolveDrop: callbacks(), onThinkingLevelChange: callbacks(), onSpeedChange: callbacks(), onSubmit: callbacks(), onOpenAttachment: callbacks(), onRemoveAttachment: callbacks() };
}
const queue = (): WebviewMessageQueue => ({ paused: true, editingId: null, items: [{ userMessageId: "X", text: "check isolation", segments: [], attachments: [], status: "queued" }] });
it.each(["normal", "queued"] as const)("shared composer surface contains attachments, input and controls (%s)", kind => {
  const attachments: WebviewMessageQueue["items"][number]["attachments"] = [{ id: "image", blobSha: "b".repeat(64), kind: "image", filename: "diagram.png", label: "diagram.png", mimeType: "image/png", thumbUri: "https://fixture/thumb", fullUri: "https://fixture/full" }];
  const props = { ...composerProps(), attachments };
  const item = { ...queue().items[0], attachments };
  render(kind === "normal"
    ? <ComposerSurface {...props} />
    : <QueuedMessageEditor item={item} sessionId="s" composerProps={props} vscodeApi={{ postMessage: vi.fn() }} action={vi.fn()} />);
  const prefix = kind === "normal" ? "" : "queue-edit-";
  const surface = screen.getByTestId(`${prefix}composer-surface`);
  expect(surface.contains(screen.getByRole("list", { name: "Pending attachments" }))).toBe(true);
  expect(surface.contains(screen.getByTestId(`${prefix}composer-input`))).toBe(true);
  expect(surface.contains(screen.getByTestId(`${prefix}composer-bar`))).toBe(true);
  expect(document.querySelectorAll(".tc-composer__surface")).toHaveLength(1);
  if (kind === "queued") expect(surface.contains(screen.getByTestId("queue-edit-cancel"))).toBe(true);
  else {
    expect(surface.querySelector(".tc-composer__header")).toBeNull();
    const notice = screen.getByTestId("composer-notices");
    const attachments = screen.getByRole("list", { name: "Pending attachments" });
    expect(notice.compareDocumentPosition(attachments) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(attachments.compareDocumentPosition(screen.getByTestId("composer-input")) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  }
});

it("attachment-only busy input keeps a disabled Send until the last attachment is removed", () => {
  const props = { ...composerProps(), allowBusyInput: true, attachments: [{ id: "a", kind: "image" as const, filename: "a.png", label: "a.png", mimeType: "image/png", blobSha: "a".repeat(64) }] };
  const view = render(<ComposerSurface {...props} />);
  expect(screen.queryByTestId("stop-button")).toBeNull();
  expect(screen.getByTestId("send-button")).toHaveProperty("disabled", true);
  view.rerender(<ComposerSurface {...props} attachments={[]} />);
  expect(screen.queryByTestId("send-button")).toBeNull();
  expect(screen.getByTestId("stop-button")).toHaveProperty("disabled", false);
});
it("empty queued editing keeps only disabled Save even with a busy background and pending main work", async () => {
  const item = { ...queue().items[0], text: "", segments: [] };
  const props = { ...composerProps(), allowBusyInput: true, attachmentsPending: true };
  render(<QueuedMessageEditor item={item} sessionId="s" composerProps={props} vscodeApi={{ postMessage: vi.fn() }} action={vi.fn()} />);
  expect(screen.queryByTestId("queue-edit-stop-button")).toBeNull();
  expect(screen.getByTestId("queue-edit-send-button")).toHaveProperty("disabled", true);
  await act(async () => { fireEvent.paste(screen.getByTestId("queue-edit-composer-input"), { clipboardData: { getData: () => "fixed" } }); });
  expect(screen.getByTestId("queue-edit-send-button")).toHaveProperty("disabled", false);
  expect(screen.getByTestId("queue-edit-cancel")).toHaveProperty("disabled", false);
});

it("missing attachments disable Save but keep the editor editable for repair", () => {
  const q = queue(); q.editingId = "X";
  q.items[0].attachments = [{ id: "gone", blobSha: "a".repeat(64), filename: "gone.png", label: "gone.png", mimeType: "image/png", kind: "image", unavailable: true }];
  render(<QueuedMessageEditor item={q.items[0]} sessionId="s" composerProps={composerProps()} vscodeApi={{ postMessage: vi.fn() }} action={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Save queued message" })).toHaveProperty("disabled", true);
  expect(screen.getByTestId("queue-edit-composer-input").getAttribute("contenteditable")).toBe("true");
});
it("paused row uses only content and necessary ARIA actions; steer row is read-only", () => {
  const postMessage = vi.fn(), q = queue(); const props = composerProps();
  const view = render(<MessageQueueDock sessionId="s" queue={q} busy composerProps={props} vscodeApi={{ postMessage }} />);
  expect(screen.getByRole("button", { name: "Collapse messages" }).textContent).toContain("1 Queued · Paused");
  fireEvent.click(screen.getByRole("button", { name: "Insert into the current task using its current mode and model" }));
  expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({ type: "queueAction", data: { sessionId: "s", userMessageId: "X", action: "send" } }));
  view.rerender(<MessageQueueDock sessionId="s" queue={{ ...q, items: [{ ...q.items[0], status: "steering" }] }} busy composerProps={props} vscodeApi={{ postMessage }} />);
  expect(screen.queryByRole("button", { name: "Delete queued message" })).toBeNull(); expect(screen.queryByRole("button", { name: "Edit queued message" })).toBeNull();
  expect(screen.getByLabelText("Sending")).toBeTruthy(); expect(screen.getByRole("button", { name: "Collapse messages" }).textContent).toContain("Messages");
});
it("composer editor keeps the screenshot header, shared config and arrow save without rewind", async () => {
  const postMessage = vi.fn(), action = vi.fn(), props = composerProps(); const q = queue();
  const view = render(<QueuedMessageEditor item={q.items[0]} sessionId="s" composerProps={props} vscodeApi={{ postMessage }} action={action} />);
  expect(screen.getByRole("region", { name: "Editing queued message" })).toBeTruthy();
  expect(screen.getByTestId("queue-edit-model-select")).toHaveProperty("disabled", true);
  expect(screen.getByTestId("queue-edit-mode-select")).toHaveProperty("disabled", true);
  view.rerender(<QueuedMessageEditor item={q.items[0]} sessionId="s" composerProps={{ ...props, busy: false }} vscodeApi={{ postMessage }} action={action} />);
  expect(screen.getByTestId("queue-edit-model-select")).toHaveProperty("disabled", false);
  expect(screen.getByTestId("queue-edit-mode-select")).toHaveProperty("disabled", false);
  expect(screen.getByRole("button", { name: "Save queued message" }).textContent).toBe("↑");
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save queued message" })); });
  expect(action).toHaveBeenCalledWith("save", expect.objectContaining({ text: "check isolation" }));
  expect(props.onDraftChange).not.toHaveBeenCalled(); expect(postMessage.mock.calls.some(([m]) => m.type === "rewindAndResend")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(action).toHaveBeenLastCalledWith("cancel");
});
it("an editing row stays highlighted in the list, with delete but no duplicate editor/send action", () => {
  const q = queue(); q.editingId = "X";
  render(<MessageQueueDock sessionId="s" queue={q} busy composerProps={composerProps()} vscodeApi={{ postMessage: vi.fn() }} />);
  expect(screen.getByTestId("queue-row").getAttribute("aria-current")).toBe("true");
  expect(screen.queryByTestId("queue-editor")).toBeNull();
  expect(screen.queryByTestId("queue-send")).toBeNull();
  expect(screen.getByTestId("queue-delete")).toBeTruthy();
});
