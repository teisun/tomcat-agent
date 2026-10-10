import { act, createEvent, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { cloneElement, createRef } from "react";
import { beforeAll, describe, expect, it, type Mock, vi } from "vitest";

import { Composer, extractDropUris, type ComposerHandle, type ComposerProps } from "./Composer";
import type { ModelPickerModel } from "./ModelPicker";
import type { Speed } from "../../../src/shared/modelSpeed";
import type { SharedSlashCommand } from "../../../src/serveClient/wire";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate, type Locale } from "../../../src/shared/i18n";

type DraftChange = (draft: {
  hasContent: boolean;
  segments: unknown[];
  text: string;
}) => void;

/**
 * jsdom omits `Blob.arrayBuffer`, which the paste path uses to get at the bytes.
 * Chromium — the only engine this code actually runs in — has had it since 2019.
 */
function pastedFile(bytes: number[], name: string, type: string): File {
  const file = new File([new Uint8Array(bytes)], name, { type });
  if (typeof file.arrayBuffer !== "function") {
    Object.defineProperty(file, "arrayBuffer", {
      value: async () => new Uint8Array(bytes).buffer,
    });
  }
  return file;
}

// jsdom has no image decoder, so the real pipeline can only ever report "no thumbnail"
// here. Stub it: these tests are about the paste plumbing, and the pixel work is covered
// against real bytes in imagePipeline.test.ts.
vi.mock("../attachments/imagePipeline", () => ({
  prepareAttachment: vi.fn(
    async (raw: { filename: string | null; mimeType: string; sourcePath?: string | null }) => ({
    dataBase64: "c3R1Yg==",
    filename: raw.filename,
    mimeType: raw.mimeType,
      sourcePath: raw.sourcePath ?? null,
    thumbBase64: "dGh1bWI=",
    warnings: [],
    }),
  ),
}));

function renderComposer({
  locale = "en",
  availableModelDetails,
  availableModels = ["gpt-5.4"],
  slashCommands = [],
  instructionCatalog = [],
  busy = false,
  allowBusyInput = false,
  hasAttachments = false,
  attachmentsPending = false,
  submitDisabled = false,
  canInterrupt = true,
  canPrompt = true,
  contextLabel = "Ctx 42%",
  contextSearchLoading = false,
  contextSearchMatches = [],
  contextSearchQuery = "",
  contextSearchTruncated = false,
  contextWindowValue,
  modelCapabilities = ["vision", "files"],
  modelValue = "gpt-5.4",
  modeValue = "plan",
  onAttachFiles = vi.fn(),
  onContextSearchClose = vi.fn(),
  onContextSearchOpen = vi.fn(),
  onContextSearchQueryChange = vi.fn(),
  onContextWindowChange = vi.fn(),
  onPickContext = vi.fn(),
  onInterrupt = vi.fn(),
  onDraftChange = vi.fn(),
  onModeChange = vi.fn(),
  onModelChange = vi.fn(),
  onOpenModelSettings = vi.fn(),
  onResolveDrop = vi.fn(),
  onThinkingLevelChange = vi.fn(),
  onSpeedChange = vi.fn(),
  onSubmit = vi.fn(),
  planState = "planning",
  supportedReasoningLevels = ["low", "medium", "high", "xhigh"],
  thinkingLevelValue = "high",
}: {
  availableModelDetails?: Record<string, ModelPickerModel>;
  locale?: Locale;
  availableModels?: string[];
  slashCommands?: SharedSlashCommand[];
  instructionCatalog?: import("../../../src/serveClient/wire").InstructionCard[];
  busy?: boolean;
  allowBusyInput?: boolean;
  hasAttachments?: boolean;
  attachmentsPending?: boolean;
  submitDisabled?: boolean;
  canInterrupt?: boolean;
  canPrompt?: boolean;
  contextLabel?: string;
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
  contextWindowValue?: number | null;
  modelCapabilities?: string[];
  modelValue?: string;
  modeValue?: "chat" | "plan";
  onAttachFiles?: (files: Array<{
    dataBase64: string;
    filename?: string | null;
    mimeType: string;
  }>) => void;
  onContextSearchClose?: () => void;
  onContextSearchOpen?: () => void;
  onContextSearchQueryChange?: (query: string) => void;
  onContextWindowChange?: (modelId: string, contextWindow: number) => void;
  onPickContext?: () => void;
  onDraftChange?: Mock<DraftChange>;
  onModeChange?: (value: "chat" | "plan") => void;
  onModelChange?: (model: string) => void;
  onOpenModelSettings?: (() => void) | null;
  onResolveDrop?: (uris: string[]) => void;
  onInterrupt?: () => void;
  onSubmit?: () => void;
  planState?: "planning" | "pending" | "executing" | "completed" | null;
  supportedReasoningLevels?: string[];
  thinkingLevelValue?: string;
  onThinkingLevelChange?: (modelId: string, value: string) => void;
  onSpeedChange?: (modelId: string, speed: Speed) => void;
} = {}) {
  const ref = createRef<ComposerHandle>();
  const element = (
    <Composer
      availableModelDetails={availableModelDetails}
      availableModels={availableModels}
      slashCommands={slashCommands}
      instructionCatalog={instructionCatalog}
      busy={busy}
      allowBusyInput={allowBusyInput}
      hasAttachments={hasAttachments}
      attachmentsPending={attachmentsPending}
      submitDisabled={submitDisabled}
      canInterrupt={canInterrupt}
      canPrompt={canPrompt}
      contextSearchLoading={contextSearchLoading}
      contextSearchMatches={contextSearchMatches}
      contextSearchQuery={contextSearchQuery}
      contextSearchTruncated={contextSearchTruncated}
      contextWindowValue={contextWindowValue}
      contextLabel={contextLabel}
      modelCapabilities={modelCapabilities}
      modeValue={modeValue}
      modelValue={modelValue}
      supportedReasoningLevels={supportedReasoningLevels}
      thinkingLevelValue={thinkingLevelValue}
      onAttachFiles={onAttachFiles}
      onContextSearchClose={onContextSearchClose}
      onContextSearchOpen={onContextSearchOpen}
      onContextSearchQueryChange={onContextSearchQueryChange}
      onContextWindowChange={onContextWindowChange}
      onPickContext={onPickContext}
      onDraftChange={onDraftChange}
      onModeChange={onModeChange}
      onModelChange={onModelChange}
      onOpenModelSettings={onOpenModelSettings ?? undefined}
      onResolveDrop={onResolveDrop}
      onThinkingLevelChange={onThinkingLevelChange}
      onSpeedChange={onSpeedChange}
      onInterrupt={onInterrupt}
      onSubmit={onSubmit}
      planState={planState}
      ref={ref}
    />
  );
  const renderResult = render(<LocaleProvider locale={locale}>{element}</LocaleProvider>);
  return {
    ...renderResult,
    rerenderWith: (props: Partial<ComposerProps>) => renderResult.rerender(<LocaleProvider locale={locale}>{cloneElement(element, props)}</LocaleProvider>),
    rerenderLocale: (next: Locale) => renderResult.rerender(<LocaleProvider locale={next}>{element}</LocaleProvider>),
    onAttachFiles,
    onDraftChange,
    onModeChange,
    onResolveDrop,
    onThinkingLevelChange,
    ref,
  };
}

describe("composer localization", () => {
  it("updates placeholder and chrome in place without losing draft, selection or mode terms", async () => {
    const f = renderComposer({ locale: "en", thinkingLevelValue: "xhigh", modeValue: "chat" });
    const input = await screen.findByTestId("composer-input");
    await waitFor(() => expect(input.querySelector("[data-placeholder]")?.getAttribute("data-placeholder")).toBe(translate("en", "composer.placeholder")));
    expect(input.getAttribute("aria-label")).toBe(translate("en", "composer.inputAria"));
    const addContext = screen.getByRole("button", { name: translate("en", "composer.addContext") });
    expect(screen.getByTestId("mode-select").textContent).toContain("Chat");
    expect(screen.getByTestId("composer-notice-plan").textContent).toBe("Plan: planning");
    expect(screen.getByTestId("model-select").textContent).toContain("gpt-5.4 Xhigh");
    expect(screen.getByTestId("context-ratio").textContent).toBe("Ctx 42%");
    await act(async () => { fireEvent.paste(input, { clipboardData: { getData: () => "unchanged draft" } }); });
    const text = input.querySelector("p")!.firstChild!;
    const range = document.createRange(); range.setStart(text, 4); range.collapse(true);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    const calls = f.onDraftChange.mock.calls.length;
    const draft = f.ref.current!.getDraft();
    f.rerenderLocale("zh-CN");
    expect(screen.getByTestId("composer-input")).toBe(input);
    expect(f.ref.current!.getDraft()).toEqual(draft);
    expect(selection.anchorNode).toBe(text); expect(selection.anchorOffset).toBe(4);
    expect(f.onDraftChange).toHaveBeenCalledTimes(calls);
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(screen.getByTestId("attachment-add")).toBe(addContext);
  });
});

describe("configuration locking", () => {
  it.each(["mode", "model"] as const)("closes the open %s menu on busy while keeping content editable", async menu => {
    const onModeChange = vi.fn(), onModelChange = vi.fn(), onThinkingLevelChange = vi.fn(), onSpeedChange = vi.fn(), onPickContext = vi.fn();
    const { ref, rerenderWith } = renderComposer({
      allowBusyInput: true, onModeChange, onModelChange, onThinkingLevelChange, onSpeedChange, onPickContext,
      availableModelDetails: { "gpt-5.4": { id: "gpt-5.4", supportedSpeeds: ["fast"], selectedSpeed: "standard" } },
    });
    await act(async () => { ref.current?.replaceDraft({ text: "keep draft", segments: [{ type: "text", text: "keep draft" }], hasContent: true }); });
    fireEvent.click(screen.getByTestId(`${menu}-select`));
    expect(screen.getByTestId(`${menu}-dropdown`)).toBeTruthy();
    if (menu === "model") {
      fireEvent.mouseEnter(document.querySelector('[data-model-id="gpt-5.4"]')!);
      fireEvent.click(screen.getByTestId("model-edit-gpt-5.4"));
      expect(screen.getByRole("button", { name: "Fast" })).toBeTruthy();
    }
    rerenderWith({ busy: true });
    for (const id of ["mode-select", "model-select"]) {
      expect(screen.getByTestId(id)).toHaveProperty("disabled", true);
      expect(screen.getByTestId(id).closest(".tc-field")?.getAttribute("title")).toBe("Cannot switch while a task is running. Change it after the task finishes.");
    }
    expect(screen.queryByTestId("mode-dropdown")).toBeNull();
    expect(screen.queryByTestId("model-dropdown")).toBeNull();
    expect(screen.queryByRole("button", { name: "Fast" })).toBeNull();
    expect(screen.queryByTestId("thinking-level-option")).toBeNull();
    expect(screen.getByTestId("composer-input").getAttribute("contenteditable")).toBe("true");
    expect(screen.getByTestId("attachment-add")).toHaveProperty("disabled", false);
    fireEvent.click(screen.getByTestId("attachment-add"));
    expect(onPickContext).toHaveBeenCalledOnce();
    for (const callback of [onModeChange, onModelChange, onThinkingLevelChange, onSpeedChange]) expect(callback).not.toHaveBeenCalled();
    rerenderWith({ busy: false, canChangeConfig: false });
    expect(screen.getByTestId("mode-select")).toHaveProperty("disabled", true);
    expect(screen.getByTestId("model-select")).toHaveProperty("disabled", true);
    rerenderWith({ busy: false, canChangeConfig: true });
    expect(screen.getByTestId("mode-select")).toHaveProperty("disabled", false);
    expect(screen.getByTestId("model-select")).toHaveProperty("disabled", false);
    expect(screen.queryByTestId("mode-dropdown")).toBeNull();
    expect(screen.queryByTestId("model-dropdown")).toBeNull();
    expect(ref.current?.getDraft().text).toBe("keep draft");
  });
});

describe("single primary action", () => {
  it.each([
    { busy: true, text: "", stop: true, disabled: false },
    { busy: true, text: "  \n ", stop: true, disabled: false },
    { busy: true, text: "next task", stop: false, disabled: false },
    { busy: false, text: "", stop: false, disabled: true },
    { busy: false, text: "next task", stop: false, disabled: false },
    { busy: true, text: "", hasAttachments: true, stop: false, disabled: true },
    { busy: true, text: "next task", submitDisabled: true, stop: false, disabled: true },
    { busy: true, text: "", attachmentsPending: true, stop: false, disabled: true },
    { busy: true, text: "next task", attachmentsPending: true, stop: false, disabled: true },
    { busy: true, text: "next task", canPrompt: false, stop: false, disabled: true },
    { busy: true, text: "", canInterrupt: false, stop: true, disabled: true },
  ])("selects one action from busy/content, independently of validity: %j", async ({ text, stop, disabled, ...props }) => {
    const onSubmit = vi.fn(), onInterrupt = vi.fn();
    const { ref, container } = renderComposer({ ...props, allowBusyInput: true, onSubmit, onInterrupt });
    await act(async () => { ref.current?.replaceDraft({ text, segments: [{ type: "text", text }], hasContent: !!text.trim() }); });
    const button = screen.getByTestId(stop ? "stop-button" : "send-button");
    expect(container.querySelectorAll(".tc-send-button")).toHaveLength(1);
    expect(button).toHaveProperty("disabled", disabled);
    expect(screen.queryByTestId(stop ? "send-button" : "stop-button")).toBeNull();
    fireEvent.keyDown(screen.getByTestId("composer-input"), { key: "Enter" });
    expect(onInterrupt).not.toHaveBeenCalled();
    expect(onSubmit).toHaveBeenCalledTimes(!stop && !disabled ? 1 : 0);
    fireEvent.click(button);
    expect(onInterrupt).toHaveBeenCalledTimes(stop && !disabled ? 1 : 0);
    expect(onSubmit).toHaveBeenCalledTimes(!stop && !disabled ? 2 : 0);
  });

  it("switches Stop → Send → Stop without resetting stopping or binding Enter to Stop", async () => {
    const onInterrupt = vi.fn(), onSubmit = vi.fn();
    const { ref } = renderComposer({ busy: true, allowBusyInput: true, onInterrupt, onSubmit });
    fireEvent.click(screen.getByTestId("stop-button"));
    expect(screen.getByTestId("stop-button").textContent).toBe("Stopping…");
    await act(async () => { ref.current?.replaceDraft({ text: "new", segments: [{ type: "text", text: "new" }], hasContent: true }); });
    fireEvent.click(screen.getByTestId("send-button"));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    await act(async () => { ref.current?.clear(); });
    expect(screen.getByTestId("stop-button")).toHaveProperty("disabled", true);
    fireEvent.keyDown(screen.getByTestId("composer-input"), { key: "Enter" });
    expect(onInterrupt).toHaveBeenCalledTimes(1);
  });

  it("counts reference and instruction atoms even when the plain text draft is empty", async () => {
    const { ref } = renderComposer({ busy: true, allowBusyInput: true });
    for (const segment of [
      { type: "reference" as const, kind: "file" as const, label: "a.ts", path: "a.ts" },
      { type: "instruction" as const, kind: "command" as const, label: "/review", resourceId: "command:.cursor/commands/review.md" },
    ]) {
      await act(async () => { ref.current?.replaceDraft({ text: "", segments: [segment], hasContent: true }); });
      expect(screen.queryByTestId("stop-button")).toBeNull();
      expect(screen.getByTestId("send-button")).toHaveProperty("disabled", false);
    }
  });
});

describe("resource slash menu", () => {
  const commands = ["reload", "install", "uninstall"].map((name) => ({name, usage:`/${name}`, summary:`${name} 说明`}));
  const paste = async (value:string) => { await act(async () => { fireEvent.paste(screen.getByTestId("composer-input"), {clipboardData:{getData:()=>value}}); }); };
  it.each(["/", "help /"])("opens at a boundary without submitting: %s", async (value) => {
    const onSubmit = vi.fn();
    renderComposer({slashCommands:commands, onSubmit});
    await paste(value);
    if (value.startsWith("help")) expect(screen.queryByTestId("slash-command-menu")).toBeNull();
    else expect(screen.getAllByTestId("slash-command-option")).toHaveLength(3);
    expect(onSubmit).not.toHaveBeenCalled();
  });
  it("chooses a command as an atom, preserves draft identity and deletes by keyboard", async () => {
    const onSubmit = vi.fn();
    const {ref} = renderComposer({onSubmit,instructionCatalog:[{id:"command:.cursor/commands/review.md",kind:"command",name:"review",description:"Check",source:".cursor",path:".cursor/commands/review.md"}]});
    await paste("/review");
    await act(async () => { fireEvent.keyDown(screen.getByTestId("composer-input"),{key:"Enter"}); });
    expect(screen.getByTestId("invocation-chip").textContent).toBe("/review");
    expect(onSubmit).not.toHaveBeenCalled();
    const draft = ref.current!.getDraft();
    expect(draft.segments[0]).toMatchObject({type:"instruction",resourceId:"command:.cursor/commands/review.md",occurrenceId:expect.any(String)});
    await act(async () => { ref.current?.replaceDraft(draft); });
    expect(ref.current!.getDraft().segments).toEqual(draft.segments);
    await act(async () => { fireEvent.mouseDown(screen.getByTestId("invocation-chip")); fireEvent.keyDown(screen.getByTestId("composer-input"),{key:"Backspace"}); });
    expect(screen.queryByTestId("invocation-chip")).toBeNull();
  });
  it("filters without case sensitivity, inserts plain text on Enter and closes on Escape", async () => {
    const onSubmit = vi.fn();
    const {ref} = renderComposer({slashCommands:commands, onSubmit});
    await paste("/UN");
    expect(screen.getAllByTestId("slash-command-option")).toHaveLength(1);
    await act(async () => { fireEvent.keyDown(screen.getByTestId("composer-input"), {key:"Enter"}); });
    expect(ref.current?.getDraft()).toMatchObject({text:"/uninstall ", segments:[{type:"text",text:"/uninstall "}]});
    expect(onSubmit).not.toHaveBeenCalled();
    await act(async () => { ref.current?.clear(); });
    await paste("/");
    await act(async () => { fireEvent.keyDown(screen.getByTestId("composer-input"), {key:"Escape"}); });
    expect(screen.queryByTestId("slash-command-menu")).toBeNull();
  });
  it.each(["src/", "https://"])("does not open in a path or URL: %s", async (value) => {
    renderComposer({slashCommands:commands}); await paste(value);
    expect(screen.queryByTestId("slash-command-menu")).toBeNull();
  });
  it("does not mount a slash menu for an old server", async () => {
    renderComposer(); await paste("/"); expect(screen.queryByTestId("slash-command-menu")).toBeNull();
  });
  it("opens slash and mention after the real Shift+Enter hardBreak", async () => {
    const onOpen = vi.fn();
    const {ref} = renderComposer({slashCommands:commands, onContextSearchOpen:onOpen});
    await paste("line");
    await act(async () => { fireEvent.keyDown(screen.getByTestId("composer-input"), {key:"Enter", shiftKey:true}); });
    expect(ref.current?.getDraft().text).toBe("line\n");
    await paste("/"); expect(screen.queryByTestId("slash-command-menu")).toBeNull();
    await act(async () => { ref.current?.clear(); });
    await paste("line");
    await act(async () => { fireEvent.keyDown(screen.getByTestId("composer-input"), {key:"Enter", shiftKey:true}); });
    await paste("@app"); expect(onOpen).toHaveBeenCalled();
    expect(screen.getByTestId("context-search-dropdown")).toBeTruthy();
  });
});

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

  it("routes Fast from the model picker without changing model or effort", () => {
    const onSpeedChange = vi.fn();
    const onModelChange = vi.fn();
    const onThinkingLevelChange = vi.fn();
    renderComposer({
      availableModelDetails: { "gpt-5.4": { id: "gpt-5.4", supportedSpeeds: ["fast"], selectedSpeed: "standard" } },
      onSpeedChange, onModelChange, onThinkingLevelChange,
    });
    fireEvent.click(screen.getByTestId("model-select"));
    fireEvent.mouseEnter(document.querySelector('[data-model-id="gpt-5.4"]')!);
    fireEvent.click(screen.getByTestId("model-edit-gpt-5.4"));
    fireEvent.click(screen.getByRole("button", { name: "Fast" }));
    expect(onSpeedChange).toHaveBeenCalledExactlyOnceWith("gpt-5.4", "fast");
    expect(onModelChange).not.toHaveBeenCalled();
    expect(onThinkingLevelChange).not.toHaveBeenCalled();
  });

  it("keeps the context slot mounted when no measurement is available", () => {
    renderComposer({ contextLabel: "" });

    expect(screen.getByTestId("context-ratio").textContent).toBe("");
  });

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
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: Hold Shift to drag files");
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

  it("uses the same model picker when model admin is unavailable", () => {
    const onModelChange = vi.fn();
    renderComposer({
      availableModels: ["gpt-5.4", "claude-opus-4-6"],
      modelValue: "gpt-5.4",
      onModelChange,
      onOpenModelSettings: null,
    });

    fireEvent.click(screen.getByTestId("model-select"));
    expect(screen.queryByTestId("model-open-settings")).toBeNull();
    fireEvent.click(screen.getAllByTestId("model-option")[1]);
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

  it("selects reasoning from the model configuration popover", () => {
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

    expect(onThinkingLevelChange).toHaveBeenCalledWith("gpt-5.4", "xhigh");
  });

  it("renders only the supported reasoning tiers in the model configuration popover", () => {
    renderComposer({
      supportedReasoningLevels: ["high", "max"],
      thinkingLevelValue: "max",
    });

    expect(screen.getByTestId("model-select").textContent).toContain("gpt-5.4 Max");
    fireEvent.click(screen.getByTestId("thinking-level-select"));
    expect(
      screen.getAllByTestId("thinking-level-option").map((node) => node.textContent),
    ).toEqual(["High", "Max"]);
  });

  it("overlays the active session context and effort onto its picker model", () => {
    renderComposer({
      availableModelDetails: {
        "gpt-5.4": {
          contextWindowOptions: [400_000, 1_000_000],
          id: "gpt-5.4",
          selectedContextWindow: 400_000,
          selectedReasoningLevel: "low",
          supportedReasoningLevels: ["low", "high"],
        },
      },
      contextWindowValue: 1_000_000,
      supportedReasoningLevels: ["low", "high"],
      thinkingLevelValue: "high",
    });

    fireEvent.click(screen.getByTestId("model-select"));
    fireEvent.mouseEnter(document.querySelector('[data-model-id="gpt-5.4"]')!);
    fireEvent.click(screen.getByTestId("model-edit-gpt-5.4"));

    expect(
      screen
        .getAllByTestId("context-window-option")
        .find((option) => option.textContent === "1M")
        ?.querySelector('[aria-label="Selected"]'),
    ).toBeTruthy();
    expect(
      screen
        .getAllByTestId("thinking-level-option")
        .find((option) => option.textContent === "High")
        ?.querySelector('[aria-label="Selected"]'),
    ).toBeTruthy();
  });

  it("keeps Edit available when the model exposes no reasoning tiers", () => {
    renderComposer({
      supportedReasoningLevels: [],
      thinkingLevelValue: "high",
    });

    fireEvent.click(screen.getByTestId("model-select"));
    const selectedOption = screen.getByTestId("model-option");
    fireEvent.mouseEnter(selectedOption.parentElement!);
    expect(screen.getByRole("button", { name: "Edit gpt-5.4" })).toBeTruthy();
  });

  it("omits the plan notice when no plan is attached", () => {
    renderComposer({ planState: null });

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
    const stopButton = screen.getByTestId("stop-button") as HTMLButtonElement;
    // Busy renders the Cursor-style solid square (CSS-drawn), not a codicon glyph.
    expect(stopButton.querySelector(".tc-stop-square")).not.toBeNull();
    expect(screen.getByTestId("stop-glyph")).toBeTruthy();
    fireEvent.click(stopButton);
    expect(onInterrupt).toHaveBeenCalledTimes(1);
    expect(stopButton.disabled).toBe(true);
    expect(stopButton.textContent).toContain("Stopping…");
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

  it("hydrates a draft without treating it as a user edit", async () => {
    const { onDraftChange, ref } = renderComposer();
    onDraftChange.mockClear();

    await act(async () => {
      ref.current?.replaceDraft({
        hasContent: true,
        segments: [{ text: "restored B draft", type: "text" }],
        text: "restored B draft",
      });
    });

    expect(ref.current?.getDraft().text).toBe("restored B draft");
    expect(screen.getByTestId("composer-input").textContent).toContain("restored B draft");
    expect(onDraftChange).not.toHaveBeenCalled();
  });

  it("inserts repeated selections with distinct occurrence identities", async () => {
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

    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(2);
    const references = ref.current!.getDraft().segments.filter((s) => s.type === "reference");
    expect(references).toHaveLength(2);
    expect(references[0].occurrenceId).not.toBe(references[1].occurrenceId);
    expect(onDraftChange).toHaveBeenCalled();
  });

  it("commits a picker batch as one draft update", async () => {
    const { onDraftChange, ref } = renderComposer();
    onDraftChange.mockClear();

    await act(async () => {
      ref.current?.insertReferences([
        {
          kind: "file",
          label: "app.ts",
          path: "src/app.ts",
          type: "reference",
        },
        {
          kind: "file",
          label: "folder/",
          path: "src/folder/",
          type: "reference",
        },
        {
          kind: "file",
          label: "app.ts",
          path: "src/app.ts",
          type: "reference",
        },
      ]);
    });

    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(3);
    expect(onDraftChange).toHaveBeenCalledTimes(1);
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
    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(3);
  });

  it("keeps same-range selections with different text and repeated re-adds", async () => {
    const { ref } = renderComposer();
    const makeReference = (text: string) => ({
      kind: "selection" as const,
      label: "notes.plan.md:20",
      lineEnd: 20,
      lineStart: 20,
      path: "plans/notes.plan.md",
      text,
      type: "reference" as const,
    });

    await act(async () => {
      ref.current?.insertReference(makeReference("first list item"));
      ref.current?.insertReference(makeReference("second list item"));
      ref.current?.insertReference(makeReference("first list item"));
    });

    const chips = screen.getAllByTestId("composer-reference-chip");
    expect(chips).toHaveLength(3);
    expect(chips.every((chip) => chip.getAttribute("title") === "plans/notes.plan.md:20")).toBe(true);
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

    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: Hold Shift to drag files");

    fireEvent.dragOver(surface, { dataTransfer });
    expect(surface.className).toContain("tc-composer__surface--drop-active");
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Release to add to context");

    fireEvent.drop(surface, { dataTransfer });
    expect(onResolveDrop).toHaveBeenCalledWith(["file:///workspace/src/app.ts"]);
    expect(surface.className).not.toContain("tc-composer__surface--drop-active");
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: Hold Shift to drag files");
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
    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: Hold Shift to drag files");

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

    expect(screen.getByTestId("composer-notice-drag").textContent).toBe("Tip: Hold Shift to drag files");
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
      "The current model does not support image/PDF attachments",
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
      "The current model does not support image attachments. Dropped images will stay in the pending list",
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

  it("keeps undo and redo local to the composer", async () => {
    const { ref } = renderComposer();
    const textbox = screen.getByTestId("composer-input");
    const outerKeydownListener = vi.fn();
    window.addEventListener("keydown", outerKeydownListener);

    try {
      await act(async () => {
        fireEvent.paste(textbox, {
          clipboardData: {
            getData: (type: string) => (type === "text/plain" ? "undo me" : ""),
          },
        });
      });
      expect(ref.current?.getDraft().text).toBe("undo me");

      fireEvent.keyDown(textbox, { ctrlKey: true, key: "z" });
      expect(ref.current?.getDraft().text).toBe("");
      expect(outerKeydownListener).not.toHaveBeenCalled();

      fireEvent.keyDown(textbox, { ctrlKey: true, key: "y" });
      expect(ref.current?.getDraft().text).toBe("undo me");
      expect(outerKeydownListener).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", outerKeydownListener);
    }
  });

  it.each([
    { key: "z", metaKey: true, shiftKey: false },
    { key: "Z", metaKey: true, shiftKey: true },
    { ctrlKey: true, key: "z", shiftKey: false },
    { ctrlKey: true, key: "y", shiftKey: false },
  ])("does not let $key undo or redo shortcut reach the webview window", (keyboardEvent) => {
    renderComposer();
    const outerKeydownListener = vi.fn();
    window.addEventListener("keydown", outerKeydownListener);

    try {
      fireEvent.keyDown(screen.getByTestId("composer-input"), keyboardEvent);
      expect(outerKeydownListener).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", outerKeydownListener);
    }
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

  it("allows repeated @ mentions, each as a separate occurrence", async () => {
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

    expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(2);
    const references = ref.current!.getDraft().segments.filter((s) => s.type === "reference");
    expect(references.every((s) => s.kind === "file" && s.lineStart == null)).toBe(true);
    expect(references[0].occurrenceId).not.toBe(references[1].occurrenceId);
  });

  it("extracts multiple clipboard image Files without inserting placeholder text", async () => {
    const onAttachFiles = vi.fn();
    renderComposer({ onAttachFiles });
    const textbox = screen.getByTestId("composer-input");
    const files = [
      pastedFile([1, 2, 3], "one.png", "image/png"),
      pastedFile([4, 5], "two.webp", "image/webp"),
    ];
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: () => "",
        items: files.map((file) => ({
          getAsFile: () => file,
          kind: "file",
          type: file.type,
        })),
      },
    });
    await waitFor(() => expect(onAttachFiles).toHaveBeenCalledTimes(1));
    expect(onAttachFiles.mock.calls[0][0]).toEqual([
      expect.objectContaining({ filename: "one.png", mimeType: "image/png" }),
      expect.objectContaining({ filename: "two.webp", mimeType: "image/webp" }),
    ]);
    expect(textbox.textContent).not.toContain("image attachment");
  });

  it("pastes PDF and image files together through the shared attachment path", async () => {
    const onAttachFiles = vi.fn();
    renderComposer({ onAttachFiles });
    const textbox = screen.getByTestId("composer-input");
    const files = [
      pastedFile([1, 2, 3], "one.png", "image/png"),
      pastedFile([0x25, 0x50, 0x44, 0x46], "brief.pdf", "application/pdf"),
    ];
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: () => "",
        items: files.map((file) => ({
          getAsFile: () => file,
          kind: "file",
          type: file.type,
        })),
      },
    });
    await waitFor(() => expect(onAttachFiles).toHaveBeenCalledTimes(1));
    expect(onAttachFiles.mock.calls[0][0]).toEqual([
      expect.objectContaining({ filename: "one.png", mimeType: "image/png" }),
      expect.objectContaining({ filename: "brief.pdf", mimeType: "application/pdf" }),
    ]);
  });

  it("preserves a clipboard file path when Chromium exposes one", async () => {
    const onAttachFiles = vi.fn();
    renderComposer({ onAttachFiles });
    const textbox = screen.getByTestId("composer-input");
    const pdf = pastedFile([0x25, 0x50, 0x44, 0x46], "brief.pdf", "application/pdf");
    Object.defineProperty(pdf, "path", {
      configurable: true,
      value: "/workspace/docs/brief.pdf",
    });

    fireEvent.paste(textbox, {
      clipboardData: {
        getData: () => "",
        items: [
          {
            getAsFile: () => pdf,
            kind: "file",
            type: pdf.type,
          },
        ],
      },
    });

    await waitFor(() => expect(onAttachFiles).toHaveBeenCalledTimes(1));
    expect(onAttachFiles.mock.calls[0]?.[0]).toEqual([
      expect.objectContaining({
        filename: "brief.pdf",
        mimeType: "application/pdf",
        sourcePath: "/workspace/docs/brief.pdf",
      }),
    ]);
  });

  it("falls back to plain-text paste when no supported attachment mime types exist", async () => {
    renderComposer();
    const textbox = screen.getByTestId("composer-input");
    const zip = pastedFile([1, 2, 3], "archive.zip", "application/zip");
    fireEvent.paste(textbox, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? "plain fallback" : ""),
        items: [
          {
            getAsFile: () => zip,
            kind: "file",
            type: zip.type,
          },
        ],
      },
    });
    await waitFor(() => expect(textbox.textContent).toContain("plain fallback"));
  });

  it("warns when pasted images are attached to a model without vision", async () => {
    const onAttachFiles = vi.fn();
    renderComposer({ modelCapabilities: [], onAttachFiles });
    const file = pastedFile([1], "shot.png", "image/png");
    fireEvent.paste(screen.getByTestId("composer-input"), {
      clipboardData: {
        getData: () => "",
        items: [
          {
            getAsFile: () => file,
            kind: "file",
            type: file.type,
          },
        ],
      },
    });
    expect(await screen.findByText(/does not declare vision capability/)).toBeTruthy();
    await waitFor(() => expect(onAttachFiles).toHaveBeenCalledTimes(1));
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
