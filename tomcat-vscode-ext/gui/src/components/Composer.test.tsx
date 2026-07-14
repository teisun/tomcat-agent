import { act, createEvent, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { Composer, extractDropUris, type ComposerHandle } from "./Composer";

function renderComposer({
  availableModels = ["gpt-5.4"],
  busy = false,
  canInterrupt = true,
  canPrompt = true,
  contextSearchLoading = false,
  contextSearchMatches = [],
  contextSearchQuery = "",
  contextSearchTruncated = false,
  modelCapabilities = ["vision", "files"],
  modelValue = "gpt-5.4",
  modeValue = "plan",
  onContextSearchClose = vi.fn(),
  onContextSearchOpen = vi.fn(),
  onContextSearchQueryChange = vi.fn(),
  onPickContext = vi.fn(),
  onInterrupt = vi.fn(),
  onDraftChange = vi.fn(),
  onModeChange = vi.fn(),
  onModelChange = vi.fn(),
  onOpenModelSettings = vi.fn(),
  onResolveDrop = vi.fn(),
  onThinkingLevelChange = vi.fn(),
  onSubmit = vi.fn(),
  planState = "planning",
  thinkingLevelValue = "high",
}: {
  availableModels?: string[];
  busy?: boolean;
  canInterrupt?: boolean;
  canPrompt?: boolean;
  contextSearchLoading?: boolean;
  contextSearchMatches?: Array<{
    description?: string | null;
    reference: {
      kind: "file";
      label: string;
      lineEnd?: number | null;
      lineStart?: number | null;
      path: string;
      text?: string | null;
      type: "reference";
    };
  }>;
  contextSearchQuery?: string;
  contextSearchTruncated?: boolean;
  modelCapabilities?: string[];
  modelValue?: string;
  modeValue?: "chat" | "plan";
  onContextSearchClose?: () => void;
  onContextSearchOpen?: () => void;
  onContextSearchQueryChange?: (query: string) => void;
  onPickContext?: () => void;
  onDraftChange?: (draft: { hasContent: boolean; segments: unknown[]; text: string }) => void;
  onModeChange?: (value: "chat" | "plan") => void;
  onModelChange?: (model: string) => void;
  onOpenModelSettings?: (() => void) | null;
  onResolveDrop?: (uris: string[]) => void;
  onInterrupt?: () => void;
  onSubmit?: () => void;
  planState?: "chat" | "planning" | "executing";
  thinkingLevelValue?: "" | "high" | "low" | "medium" | "xhigh";
  onThinkingLevelChange?: (value: "" | "high" | "low" | "medium" | "xhigh") => void;
} = {}) {
  const ref = createRef<ComposerHandle>();
  const renderResult = render(
    <Composer
      availableModels={availableModels}
      busy={busy}
      canInterrupt={canInterrupt}
      canPrompt={canPrompt}
      contextSearchLoading={contextSearchLoading}
      contextSearchMatches={contextSearchMatches}
      contextSearchQuery={contextSearchQuery}
      contextSearchTruncated={contextSearchTruncated}
      contextLabel="Ctx 42%"
      modelCapabilities={modelCapabilities}
      modeValue={modeValue}
      modelValue={modelValue}
      thinkingLevelValue={thinkingLevelValue}
      onContextSearchClose={onContextSearchClose}
      onContextSearchOpen={onContextSearchOpen}
      onContextSearchQueryChange={onContextSearchQueryChange}
      onPickContext={onPickContext}
      onDraftChange={onDraftChange}
      onModeChange={onModeChange}
      onModelChange={onModelChange}
      onOpenModelSettings={onOpenModelSettings ?? undefined}
      onResolveDrop={onResolveDrop}
      onThinkingLevelChange={onThinkingLevelChange}
      onInterrupt={onInterrupt}
      onSubmit={onSubmit}
      planState={planState}
      ref={ref}
    />,
  );
  return {
    ...renderResult,
    onDraftChange,
    onModeChange,
    onResolveDrop,
    onThinkingLevelChange,
    ref,
  };
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
  Object.defineProperty(Document.prototype, "elementFromPoint", {
    configurable: true,
    value: () => document.body,
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});

describe("Composer", () => {
  const searchMatch = {
    description: "src",
    reference: {
      kind: "file" as const,
      label: "app.ts",
      lineEnd: null,
      lineStart: null,
      path: "src/app.ts",
      text: null,
      type: "reference" as const,
    },
  };

  it("renders plan status in the notice rail instead of the control bar", () => {
    const { container } = renderComposer();

    expect(screen.getByTestId("composer-notice-plan").textContent).toBe("Plan: planning");
    expect(container.querySelector(".tc-composer__bar .tc-notice--plan")).toBeNull();
    expect(screen.queryByText("Tomcat is responding...")).toBeNull();
  });

  it("renders drag and plan notices on one line when both are active", () => {
    renderComposer();

    const notices = screen.getByTestId("composer-notices");
    expect(
      [...notices.children].map((node) => (node as HTMLElement).dataset.testid),
    ).toEqual(["composer-notice-drag", "composer-notice-plan"]);
    expect(screen.getByText("Tip:", { selector: "strong" }).className).toContain("tc-notice__tip");
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: 拖文件请按住 Shift");
    expect(screen.getByTestId("composer-notice-drag").className).toContain("tc-notice--left");
    expect(screen.getByTestId("composer-notice-drag").getAttribute("aria-hidden")).toBe("true");
    expect(screen.getByTestId("composer-notice-plan").className).toContain("tc-notice--right");
    expect(screen.getByTestId("composer-notice-plan").getAttribute("aria-hidden")).toBeNull();
  });

  it("opens the model menu and routes Add Models to settings", () => {
    const onOpenModelSettings = vi.fn();
    renderComposer({
      availableModels: ["gpt-5.4", "claude-opus-4-6"],
      onOpenModelSettings,
    });

    fireEvent.click(screen.getByTestId("model-select"));

    expect(screen.getAllByTestId("model-option")).toHaveLength(2);
    fireEvent.click(screen.getByTestId("model-open-settings"));
    expect(onOpenModelSettings).toHaveBeenCalledTimes(1);
  });

  it("selects a model from the custom dropdown", () => {
    const onModelChange = vi.fn();
    renderComposer({
      availableModels: ["gpt-5.4", "claude-opus-4-6"],
      modelValue: "gpt-5.4",
      onModelChange,
    });

    fireEvent.click(screen.getByTestId("model-select"));
    fireEvent.click(screen.getAllByTestId("model-option")[1]);

    expect(onModelChange).toHaveBeenCalledWith("claude-opus-4-6");
  });

  it("falls back to the native select when model admin is unavailable", () => {
    const onModelChange = vi.fn();
    renderComposer({
      availableModels: ["gpt-5.4", "claude-opus-4-6"],
      modelValue: "gpt-5.4",
      onModelChange,
      onOpenModelSettings: null,
    });

    const modelSelect = screen.getByTestId("model-select") as HTMLSelectElement;
    expect(modelSelect.tagName).toBe("SELECT");
    expect(screen.queryByTestId("model-open-settings")).toBeNull();

    fireEvent.change(modelSelect, { target: { value: "claude-opus-4-6" } });
    expect(onModelChange).toHaveBeenCalledWith("claude-opus-4-6");
  });

  it("selects chat mode from the custom dropdown", () => {
    const onModeChange = vi.fn();
    renderComposer({
      modeValue: "plan",
      onModeChange,
    });

    fireEvent.click(screen.getByTestId("mode-select"));
    fireEvent.click(
      screen.getAllByTestId("mode-option").find((node) => node.textContent === "Chat") ??
        screen.getAllByTestId("mode-option")[0],
    );

    expect(onModeChange).toHaveBeenCalledWith("chat");
  });

  it("selects the reasoning effort from the custom dropdown", () => {
    const onThinkingLevelChange = vi.fn();
    renderComposer({
      onThinkingLevelChange,
      thinkingLevelValue: "high",
    });

    fireEvent.click(screen.getByTestId("thinking-level-select"));
    fireEvent.click(
      screen
        .getAllByTestId("thinking-level-option")
        .find((node) => node.textContent === "Xhigh") ??
        screen.getAllByTestId("thinking-level-option")[0],
    );

    expect(onThinkingLevelChange).toHaveBeenCalledWith("xhigh");
  });

  it("omits the plan notice when chat mode is active", () => {
    renderComposer({ planState: "chat" });

    expect(screen.queryByTestId("composer-notice-plan")).toBeNull();
  });

  it("swaps the send button for a stop button while busy", () => {
    const onInterrupt = vi.fn();
    renderComposer({
      busy: true,
      canInterrupt: true,
      canPrompt: true,
      onInterrupt,
    });

    expect(screen.queryByTestId("send-button")).toBeNull();
    const stopButton = screen.getByTestId("stop-button");
    // Busy renders the Cursor-style solid square (CSS-drawn), not a codicon glyph.
    expect(stopButton.querySelector(".tc-stop-square")).not.toBeNull();
    expect(screen.getByTestId("stop-glyph")).toBeTruthy();
    fireEvent.click(stopButton);
    expect(onInterrupt).toHaveBeenCalledTimes(1);
  });

  it("disables the stop button when interrupt is not allowed", () => {
    const onInterrupt = vi.fn();
    renderComposer({
      busy: true,
      canInterrupt: false,
      canPrompt: false,
      onInterrupt,
    });

    const stopButton = screen.getByTestId("stop-button") as HTMLButtonElement;
    expect(stopButton.disabled).toBe(true);
    fireEvent.click(stopButton);
    expect(onInterrupt).not.toHaveBeenCalled();
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

  it("keeps distinct line-less selections from the same file as separate chips", async () => {
    const { ref } = renderComposer();

    await act(async () => {
      ref.current?.insertReference({
        kind: "selection",
        label: "notes.plan.md",
        lineEnd: null,
        lineStart: null,
        path: "plans/notes.plan.md",
        text: "first selected snippet",
        type: "reference",
      });
      ref.current?.insertReference({
        kind: "selection",
        label: "notes.plan.md",
        lineEnd: null,
        lineStart: null,
        path: "plans/notes.plan.md",
        text: "a totally different snippet",
        type: "reference",
      });
    });

    // Both distinct snippets must survive; only exact re-adds should dedupe.
    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(2);

    await act(async () => {
      ref.current?.insertReference({
        kind: "selection",
        label: "notes.plan.md",
        lineEnd: null,
        lineStart: null,
        path: "plans/notes.plan.md",
        text: "first selected snippet",
        type: "reference",
      });
    });
    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(2);
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

    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: 拖文件请按住 Shift");

    fireEvent.dragOver(surface, { dataTransfer });
    expect(surface.className).toContain("tc-composer__surface--drop-active");
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("松手加入上下文");

    fireEvent.drop(surface, { dataTransfer });
    expect(onResolveDrop).toHaveBeenCalledWith(["file:///workspace/src/app.ts"]);
    expect(surface.className).not.toContain("tc-composer__surface--drop-active");
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: 拖文件请按住 Shift");
  });

  it("prevents default on dragenter and keeps the Shift hint even after content exists", () => {
    const { ref } = renderComposer();
    const surface = screen.getByTestId("composer-surface");
    const enterEvent = createEvent.dragEnter(surface, {
      dataTransfer: {
        files: [],
        getData: () => "",
      },
    });

    fireEvent(surface, enterEvent);
    expect(enterEvent.defaultPrevented).toBe(true);
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: 拖文件请按住 Shift");

    act(() => {
      ref.current?.insertReference({
        kind: "file",
        label: "app.ts",
        lineEnd: null,
        lineStart: null,
        path: "app.ts",
        text: null,
        type: "reference",
      });
    });

    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: 拖文件请按住 Shift");
  });

  it("suppresses raw editor drops and forwards file uris once", () => {
    const { onResolveDrop, ref } = renderComposer();
    const textbox = screen.getByTestId("composer-input");
    const dataTransfer = {
      files: [],
      getData(type: string) {
        if (type === "text/plain") {
          return "file:///workspace/src/app.ts";
        }
        if (type === "text/uri-list") {
          return "file:///workspace/src/app.ts";
        }
        return "";
      },
    } as unknown as DataTransfer;

    fireEvent.drop(textbox, { dataTransfer });

    expect(onResolveDrop).toHaveBeenCalledTimes(1);
    expect(onResolveDrop).toHaveBeenCalledWith(["file:///workspace/src/app.ts"]);
    expect(ref.current?.getDraft()).toEqual({
      hasContent: false,
      segments: [],
      text: "",
    });
  });

  it("lets capability warnings take over the single-line notice rail", () => {
    const onPickContext = vi.fn();
    renderComposer({
      modelCapabilities: ["reasoning"],
      onPickContext,
    });

    fireEvent.click(screen.getByTestId("attachment-add"));

    expect(onPickContext).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("composer-notice-capability").textContent).toContain(
      "当前模型不支持图片/PDF 附件",
    );
    expect(screen.queryByTestId("composer-notice-drag")).toBeNull();
    expect(screen.queryByTestId("composer-notice-plan")).toBeNull();
    expect(
      [...screen.getByTestId("composer-notices").children].map(
        (node) => (node as HTMLElement).dataset.testid,
      ),
    ).toEqual(["composer-notice-capability"]);
  });

  it("warns when unsupported image drops still add an attachment", () => {
    const { onResolveDrop } = renderComposer({
      modelCapabilities: ["files"],
    });
    const surface = screen.getByTestId("composer-surface");
    const dataTransfer = {
      files: [],
      getData(type: string) {
        if (type === "text/uri-list") {
          return "file:///workspace/assets/mockup.png";
        }
        return "";
      },
    } as unknown as DataTransfer;

    fireEvent.drop(surface, { dataTransfer });

    expect(onResolveDrop).toHaveBeenCalledWith(["file:///workspace/assets/mockup.png"]);
    expect(screen.getByTestId("composer-notice-capability").textContent).toContain(
      "当前模型不支持图片附件；拖入后会先加入待发送列表",
    );
    expect(screen.queryByTestId("composer-notice-drag")).toBeNull();
    expect(screen.queryByTestId("composer-notice-plan")).toBeNull();
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

  it("opens @ context search and forwards raw query updates", async () => {
    const onContextSearchOpen = vi.fn();
    const onContextSearchQueryChange = vi.fn();
    renderComposer({
      onContextSearchOpen,
      onContextSearchQueryChange,
    });
    const textbox = screen.getByTestId("composer-input");

    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app" : ""),
        },
      });
    });

    expect(onContextSearchOpen).toHaveBeenCalledTimes(1);
    expect(onContextSearchQueryChange).toHaveBeenLastCalledWith("app");
  });

  it("exposes closeMention and closes an active @ session through the plugin", async () => {
    const { ref } = renderComposer({
      contextSearchMatches: [searchMatch],
      contextSearchQuery: "app",
    });
    const textbox = screen.getByTestId("composer-input");

    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app" : ""),
        },
      });
    });

    expect(screen.getByTestId("context-search-dropdown")).toBeTruthy();

    await act(async () => {
      ref.current?.closeMention();
    });

    expect(screen.queryByTestId("context-search-dropdown")).toBeNull();
  });

  it("does not trigger @ context search during IME composition", async () => {
    const onContextSearchOpen = vi.fn();
    const onContextSearchQueryChange = vi.fn();
    renderComposer({
      onContextSearchOpen,
      onContextSearchQueryChange,
    });
    const textbox = screen.getByTestId("composer-input");

    fireEvent.compositionStart(textbox);
    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app" : ""),
        },
      });
    });

    expect(onContextSearchOpen).not.toHaveBeenCalled();
    expect(onContextSearchQueryChange).not.toHaveBeenCalled();
  });

  it("lets Enter select an @ match instead of submitting", async () => {
    const onSubmit = vi.fn();
    renderComposer({
      contextSearchMatches: [searchMatch],
      contextSearchQuery: "app",
      onSubmit,
    });
    const textbox = screen.getByTestId("composer-input");

    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app" : ""),
        },
      });
    });

    expect(screen.getByTestId("context-search-dropdown")).toBeTruthy();

    fireEvent.keyDown(textbox, { key: "Enter" });

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByTestId("composer-reference-chip").textContent).toContain("app.ts");
  });

  it("deduplicates @ selections and keeps them as file references without line numbers", async () => {
    const { onDraftChange, ref } = renderComposer({
      contextSearchMatches: [searchMatch],
      contextSearchQuery: "app.ts:12",
    });
    const textbox = screen.getByTestId("composer-input");

    act(() => {
      ref.current?.insertReference(searchMatch.reference);
    });

    await act(async () => {
      fireEvent.paste(textbox, {
        clipboardData: {
          getData: (type: string) => (type === "text/plain" ? "@app.ts:12" : ""),
        },
      });
    });
    fireEvent.keyDown(textbox, { key: "Enter" });

    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(1);
    expect(onDraftChange).toHaveBeenLastCalledWith({
      hasContent: true,
      segments: [
        {
          kind: "file",
          label: "app.ts",
          lineEnd: null,
          lineStart: null,
          path: "src/app.ts",
          text: null,
          type: "reference",
        },
        {
          text: " ",
          type: "text",
        },
      ],
      text: "app.ts ",
    });
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
