import { act, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { Composer, extractDropUris, type ComposerHandle } from "./Composer";

function renderComposer({
  busy = false,
  canPrompt = true,
  onInterrupt = vi.fn(),
  onDraftChange = vi.fn(),
  onResolveDrop = vi.fn(),
  onSubmit = vi.fn(),
  planState = "planning",
}: {
  busy?: boolean;
  canPrompt?: boolean;
  onDraftChange?: (draft: { hasContent: boolean; segments: unknown[]; text: string }) => void;
  onResolveDrop?: (uris: string[]) => void;
  onInterrupt?: () => void;
  onSubmit?: () => void;
  planState?: "chat" | "planning" | "executing";
} = {}) {
  const ref = createRef<ComposerHandle>();
  const renderResult = render(
    <Composer
      availableModels={["gpt-5.4"]}
      busy={busy}
      canPrompt={canPrompt}
      contextLabel="Ctx 42%"
      modeValue="plan"
      modelValue="gpt-5.4"
      thinkingLevelValue="high"
      onAddAttachment={vi.fn()}
      onDraftChange={onDraftChange}
      onModeChange={vi.fn()}
      onModelChange={vi.fn()}
      onResolveDrop={onResolveDrop}
      onThinkingLevelChange={vi.fn()}
      onInterrupt={onInterrupt}
      onSubmit={onSubmit}
      planState={planState}
      ref={ref}
    />,
  );
  return { ...renderResult, onDraftChange, onResolveDrop, ref };
}

beforeAll(() => {
  const emptyRect = () => ({
    bottom: 0,
    height: 0,
    left: 0,
    right: 0,
    toJSON() {
      return {};
    },
    top: 0,
    width: 0,
    x: 0,
    y: 0,
  });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: emptyRect,
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});

describe("Composer", () => {
  it("renders plan status in the footer instead of the control bar", () => {
    const { container } = renderComposer();

    expect(screen.getByTestId("composer-plan-status-footer").textContent).toBe("Plan: planning");
    expect(container.querySelector(".tc-composer__bar .tc-composer__plan-status")).toBeNull();
    expect(screen.queryByText("Tomcat is responding...")).toBeNull();
  });

  it("omits the footer status when chat mode is active", () => {
    renderComposer({ planState: "chat" });

    expect(screen.queryByTestId("composer-plan-status-footer")).toBeNull();
  });

  it("swaps the send button for a stop button while busy", () => {
    const onInterrupt = vi.fn();
    renderComposer({
      busy: true,
      canPrompt: true,
      onInterrupt,
    });

    expect(screen.queryByTestId("send-button")).toBeNull();
    fireEvent.click(screen.getByTestId("stop-button"));
    expect(onInterrupt).toHaveBeenCalledTimes(1);
  });

  it("inserts references as inline chips and deduplicates them", async () => {
    const { onDraftChange, ref } = renderComposer();

    await act(async () => {
      ref.current?.insertReference({
        kind: "selection",
        label: "app.ts:3-5",
        lineEnd: 5,
        lineStart: 3,
        path: "app.ts",
        text: "const answer = 42;",
        type: "reference",
      });
      ref.current?.insertReference({
        kind: "selection",
        label: "app.ts:3-5",
        lineEnd: 5,
        lineStart: 3,
        path: "app.ts",
        text: "const answer = 42;",
        type: "reference",
      });
    });

    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(1);
    expect(onDraftChange).toHaveBeenLastCalledWith({
      hasContent: true,
      segments: [
        {
          kind: "selection",
          label: "app.ts:3-5",
          lineEnd: 5,
          lineStart: 3,
          path: "app.ts",
          text: "const answer = 42;",
          type: "reference",
        },
        {
          text: " ",
          type: "text",
        },
      ],
      text: "app.ts:3-5 ",
    });
  });

  it("extracts drop uris across vscode mime variants without duplicates", () => {
    const file = Object.assign(new File([""], "local.ts"), {
      path: "/workspace/from-file.ts",
    });
    const dataTransfer = {
      files: [file],
      getData(type: string) {
        switch (type) {
          case "resourceurls":
            return JSON.stringify(["file:///workspace/a.ts"]);
          case "application/vnd.code.uri-list":
            return "file:///workspace/b.ts";
          case "CodeFiles":
            return JSON.stringify(["file:///workspace/c.ts"]);
          case "text/uri-list":
            return "file:///workspace/a.ts\nfile:///workspace/d.ts";
          default:
            return "";
        }
      },
    } as unknown as DataTransfer;

    expect(extractDropUris(dataTransfer)).toEqual([
      "file:///workspace/a.ts",
      "file:///workspace/b.ts",
      "file:///workspace/c.ts",
      "file:///workspace/d.ts",
      "file:///workspace/from-file.ts",
    ]);
  });

  it("highlights drop targets and resolves dropped uris", () => {
    const { onResolveDrop } = renderComposer();
    const surface = screen.getByTestId("composer-surface");
    const dataTransfer = {
      files: [],
      getData(type: string) {
        if (type === "text/uri-list") {
          return "file:///workspace/src/app.ts";
        }
        return "";
      },
    } as unknown as DataTransfer;

    fireEvent.dragOver(surface, { dataTransfer });
    expect(surface.className).toContain("tc-composer__surface--drop-active");

    fireEvent.drop(surface, { dataTransfer });
    expect(onResolveDrop).toHaveBeenCalledWith(["file:///workspace/src/app.ts"]);
    expect(surface.className).not.toContain("tc-composer__surface--drop-active");
  });

  it("does not submit on Shift+Enter or during IME composition", () => {
    const onSubmit = vi.fn();
    renderComposer({ onSubmit });
    const textbox = screen.getByTestId("composer-input");

    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "hello composer" : ""),
      },
    });

    fireEvent.keyDown(textbox, { key: "Enter", shiftKey: true });
    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.compositionStart(textbox);
    fireEvent.keyDown(textbox, { key: "Enter" });
    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.compositionEnd(textbox);
    fireEvent.keyDown(textbox, { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("does not submit when the browser still marks Enter as composing", () => {
    const onSubmit = vi.fn();
    renderComposer({ onSubmit });
    const textbox = screen.getByTestId("composer-input");

    fireEvent.keyDown(textbox, { isComposing: true, key: "Enter" });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("clears drop highlighting on dragend", () => {
    renderComposer();
    const surface = screen.getByTestId("composer-surface");
    const dataTransfer = {
      files: [],
      getData() {
        return "";
      },
    } as unknown as DataTransfer;

    fireEvent.dragOver(surface, { dataTransfer });
    expect(surface.className).toContain("tc-composer__surface--drop-active");

    fireEvent.dragEnd(surface);
    expect(surface.className).not.toContain("tc-composer__surface--drop-active");
  });
});
