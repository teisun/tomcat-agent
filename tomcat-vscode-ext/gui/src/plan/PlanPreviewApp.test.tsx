import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  PlanPreviewDomAction,
  PlanPreviewIntent,
  PlanPreviewStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/planPreviewProtocol";
import type { PathResolution } from "../../../src/shared/pathResolution";
import { PlanPreviewApp } from "./PlanPreviewApp";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";

function makeState(overrides: Partial<PlanPreviewStateSnapshot> = {}): PlanPreviewStateSnapshot {
  return {
    availableModelDetails: {},
    availableModels: ["gpt-5.6", "claude-opus"],
    // 6 body lines → mapped to arbitrary absolute file lines 10..15.
    bodyLineMap: [10, 11, 12, 13, 14, 15],
    bodyMarkdown: "# Plan Heading\n\nSome **bold** intro with `code`.\n\n- item one\n- item two",
    buildModel: "",
    canBuild: true,
    overview: "OVERVIEW_SHOULD_NOT_RENDER",
    path: "/home/u/.tomcat/plans/demo.plan.md",
    planId: "plan-1",
    raw: "---\nname: X\n---\n# Plan Heading",
    sessionModel: "gpt-5.6",
    state: "planning",
    title: "TITLE_SHOULD_NOT_RENDER",
    todos: [
      { content: "Pending item", id: "t1", status: "pending" },
      { content: "In progress item", id: "t2", status: "in_progress" },
      { content: "Done item", id: "t3", status: "completed" },
    ],
    toolbarStyle: "native",
    ...overrides,
  };
}

function pushState(state: PlanPreviewStateSnapshot): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: { channel: "state", content: state, messageId: "state-1" },
      }),
    );
  });
}

function pushCaptureSelectionEvent(): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: {
          channel: "event",
          content: { type: "captureSelectionForChat" },
          messageId: "evt-1",
        },
      }),
    );
  });
}

function pushCaptureDomEvent(messageId = "dom-1"): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: {
          channel: "event",
          content: { type: "__test.capture_dom" },
          messageId,
        },
      }),
    );
  });
}

function pushDomActionEvent(action: PlanPreviewDomAction): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: {
          channel: "event",
          content: { action, type: "__test.dom_action" },
          messageId: "dom-action-1",
        },
      }),
    );
  });
}

function pushPathsResolvedEvent(requestId: string, results: PathResolution[]): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: {
          channel: "event",
          content: { requestId, results, type: "pathsResolved" },
          messageId: "paths-resolved",
        },
      }),
    );
  });
}

function mockSelectionText(text: string): void {
  vi.spyOn(window, "getSelection").mockReturnValue({
    toString: () => text,
  } as unknown as Selection);
}

/** Mock a live selection whose range spans the contents of `element`. */
function mockSelectionOn(element: Element, text: string): void {
  const range = document.createRange();
  range.selectNodeContents(element);
  vi.spyOn(window, "getSelection").mockReturnValue({
    getRangeAt: () => range,
    rangeCount: 1,
    toString: () => text,
  } as unknown as Selection);
}

function makeApi(): VsCodeApiLike<PlanPreviewIntent> & { postMessage: ReturnType<typeof vi.fn> } {
  return { postMessage: vi.fn() };
}

function intentsOfType(
  api: { postMessage: ReturnType<typeof vi.fn> },
  type: PlanPreviewIntent["type"],
): PlanPreviewIntent[] {
  return api.postMessage.mock.calls
    .map((call) => call[0] as PlanPreviewIntent)
    .filter((intent) => intent.type === type);
}

describe("PlanPreviewApp", () => {
  it("switches labels without replacing Markdown or losing selection, scroll and Find query", async () => {
    const api = makeApi();
    const view = render(<LocaleProvider locale="en"><PlanPreviewApp vscodeApi={api} /></LocaleProvider>);
    pushState(makeState({ toolbarStyle: "hybrid", bodyMarkdown: "# Kept heading\n\n```ts\nconst stable = 1;\n```" }));
    const content = screen.getByTestId("plan-content");
    expect(screen.getByRole("main", { name: translate("en", "plan.preview") })).toBe(content);
    const todoCount = screen.getByTestId("plan-todos-count");
    expect(todoCount.textContent).toBe(translate("en", "plan.todos.other", { count: 3 }));
    const code = screen.getByTestId("plan-markdown-body").querySelector("code")!;
    const heading = screen.getByRole("heading", { name: "Kept heading" });
    content.scrollTop = 120;
    const range = document.createRange(); range.selectNodeContents(heading);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    view.rerender(<LocaleProvider locale="zh-CN"><PlanPreviewApp vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("plan-content")).toBe(content);
    expect(screen.getByTestId("plan-markdown-body").querySelector("code")).toBe(code);
    expect(selection.toString()).toBe("Kept heading");
    expect(content.scrollTop).toBe(120);
    expect(screen.getByTestId("plan-todos-count")).toBe(todoCount);
    pushDomActionEvent({ kind: "setFindQuery", query: "stable" });
    await waitFor(() => expect((screen.getByTestId("plan-find-input") as HTMLInputElement).value).toBe("stable"));
    view.rerender(<LocaleProvider locale="en"><PlanPreviewApp vscodeApi={api} /></LocaleProvider>);
    expect((screen.getByTestId("plan-find-input") as HTMLInputElement).value).toBe("stable");
    expect(screen.getByTestId("plan-markdown-body").querySelector("code")).toBe(code);
    expect(intentsOfType(api, "plan.ready")).toHaveLength(1);
  });

  it("sends plan.ready on mount and shows a loading state until a frame arrives", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    expect(screen.getByTestId("plan-loading")).toBeTruthy();
    expect(intentsOfType(api, "plan.ready")).toHaveLength(1);
  });

  it("reports each received state frame in a test DOM snapshot", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());
    pushState(makeState({ bodyMarkdown: "# Updated" }));
    pushCaptureDomEvent();

    const snapshotReply = api.postMessage.mock.calls
      .map((call) => call[0] as { data?: { refreshCounters?: { webviewStateFrames?: number } }; type?: string })
      .find((message) => message.type === "__test.dom_snapshot");
    expect(snapshotReply?.data?.refreshCounters?.webviewStateFrames).toBe(2);
  });

  it("renders body → N To-dos → divider → four-state checklist in that order", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    const body = screen.getByTestId("plan-markdown-body");
    const count = screen.getByTestId("plan-todos-count");
    const divider = document.querySelector(".tc-plan-preview__divider");
    const list = screen.getByTestId("plan-todo-list");

    expect(count.textContent).toBe("3 To-dos");
    expect(divider).not.toBeNull();
    expect(body.compareDocumentPosition(count) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(count.compareDocumentPosition(divider as Node) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect((divider as Node).compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getAllByTestId("plan-todo-item")).toHaveLength(3);
  });

  it("exposes one labelled main region and associates the todo heading with its list", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    const main = screen.getByRole("main", { name: "Plan preview" });
    const heading = screen.getByRole("heading", { level: 2, name: "3 To-dos" });
    const list = screen.getByRole("list", { name: "3 To-dos" });

    expect(main.contains(heading)).toBe(true);
    expect(main.contains(list)).toBe(true);
    expect(list.getAttribute("aria-labelledby")).toBe(heading.id);
  });

  it("keeps only rendered body and todo copy in the in-webview Find search surface", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(
      makeState({
        bodyMarkdown: "# BODY_FIND_TOKEN",
        overview: "HIDDEN_OVERVIEW_FIND_TOKEN",
        raw: "---\nsecret: HIDDEN_FRONTMATTER_FIND_TOKEN\n---\n# BODY_FIND_TOKEN",
        title: "HIDDEN_TITLE_FIND_TOKEN",
        todos: [{ content: "TODO_FIND_TOKEN", id: "find", status: "pending" }],
      }),
    );

    const visibleText = screen.getByRole("main", { name: "Plan preview" }).textContent ?? "";
    expect(visibleText).toContain("BODY_FIND_TOKEN");
    expect(visibleText).toContain("TODO_FIND_TOKEN");
    expect(visibleText).not.toContain("HIDDEN_OVERVIEW_FIND_TOKEN");
    expect(visibleText).not.toContain("HIDDEN_FRONTMATTER_FIND_TOKEN");
    expect(visibleText).not.toContain("HIDDEN_TITLE_FIND_TOKEN");
  });

  it("does not render the frontmatter title or overview in the preview", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());
    expect(screen.queryByText("TITLE_SHOULD_NOT_RENDER")).toBeNull();
    expect(screen.queryByText("OVERVIEW_SHOULD_NOT_RENDER")).toBeNull();
  });

  it("pluralizes the count for one and zero todos", () => {
    const api = makeApi();
    const { rerender } = render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ todos: [{ content: "only", id: "x", status: "pending" }] }));
    expect(screen.getByTestId("plan-todos-count").textContent).toBe("1 To-do");
    rerender(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ todos: [] }));
    expect(screen.getByTestId("plan-todos-count").textContent).toBe("0 To-dos");
  });

  it("always renders the preview (there is no in-webview markdown/source view)", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());
    expect(screen.getByTestId("plan-markdown-body")).toBeTruthy();
    expect(screen.queryByTestId("plan-source")).toBeNull();
    expect(screen.queryByTestId("plan-open-editor")).toBeNull();
  });

  it("keeps the hybrid action strip outside the scrolling content column (fixed header)", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ toolbarStyle: "hybrid" }));

    const strip = screen.getByTestId("plan-action-strip");
    const content = screen.getByTestId("plan-content");
    // Sibling, not nested: the header never scrolls with the body.
    expect(content.contains(strip)).toBe(false);
    expect(strip.parentElement?.classList.contains("tc-plan-preview")).toBe(true);
    expect(
      strip.compareDocumentPosition(content) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("shows no in-body action controls in native toolbar style", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ toolbarStyle: "native" }));
    expect(screen.queryByTestId("plan-action-strip")).toBeNull();
    expect(screen.queryByTestId("plan-build")).toBeNull();
    expect(screen.queryByTestId("plan-build-model-select")).toBeNull();
  });

  it("renders the hybrid action strip with a yellow Build button that emits build", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ toolbarStyle: "hybrid" }));

    const strip = screen.getByTestId("plan-action-strip");
    expect(strip).toBeTruthy();
    const build = screen.getByTestId("plan-build");
    expect(build.classList.contains("tc-plan-build-button")).toBe(true);

    fireEvent.click(build);
    expect(intentsOfType(api, "build")).toHaveLength(1);
    expect(intentsOfType(api, "build")[0]).not.toHaveProperty("data");
  });

  it("disables the hybrid Build button when canBuild is false", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ canBuild: false, toolbarStyle: "hybrid" }));
    expect((screen.getByTestId("plan-build") as HTMLButtonElement).disabled).toBe(true);
  });

  it("sends setBuildModel when the hybrid model dropdown changes", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ toolbarStyle: "hybrid" }));
    fireEvent.click(screen.getByTestId("plan-build-model-select"));
    fireEvent.click(screen.getAllByTestId("model-option")[1]);
    const intents = intentsOfType(api, "setBuildModel");
    expect(intents).toHaveLength(1);
    expect((intents[0] as { data: { modelId: string } }).data.modelId).toBe("claude-opus");
  });

  it("emits setSpeed from the hybrid picker without switching the build model", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({
      availableModelDetails: { "gpt-5.6": { capabilities: [], contextWindowOptions: [], supportedReasoningLevels: [], id: "gpt-5.6", supportedSpeeds: ["fast", "ultrafast"], selectedSpeed: "standard" } },
      toolbarStyle: "hybrid",
    }));
    fireEvent.click(screen.getByTestId("plan-build-model-select"));
    fireEvent.mouseEnter(document.querySelector('[data-model-id="gpt-5.6"]')!);
    fireEvent.click(screen.getByTestId("model-edit-gpt-5.6"));
    fireEvent.click(screen.getByRole("button", { name: "Ultrafast" }));
    expect(intentsOfType(api, "setSpeed")).toEqual([expect.objectContaining({ data: { modelId: "gpt-5.6", speed: "ultrafast" } })]);
    expect(intentsOfType(api, "setBuildModel")).toHaveLength(0);
  });

  it("overlays the session's context and effort in the hybrid model picker", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(
      makeState({
        availableModelDetails: {
          "gpt-5.6": {
            capabilities: [],
            contextWindowOptions: [400_000, 1_000_000],
            id: "gpt-5.6",
            selectedContextWindow: 1_000_000,
            selectedReasoningLevel: "high",
            supportedReasoningLevels: ["low", "high"],
          },
        },
        toolbarStyle: "hybrid",
      }),
    );

    fireEvent.click(screen.getByTestId("plan-build-model-select"));
    fireEvent.mouseEnter(document.querySelector('[data-model-id="gpt-5.6"]')!);
    fireEvent.click(screen.getByTestId("model-edit-gpt-5.6"));

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

  it("resolves inline plan paths before sending openFile", async () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(
      makeState({
        bodyMarkdown: "Check `src/test/fixtures/plan-preview.ts:18` before shipping.",
      }),
    );
    const resolveIntent = intentsOfType(api, "resolvePaths")[0];
    expect(resolveIntent).toBeTruthy();
    const requestId = (resolveIntent as { data: { requestId: string } }).data.requestId;
    expect(screen.queryByTestId("assistant-clickable-path")).toBeNull();

    pushPathsResolvedEvent(requestId, [
      {
        kind: "file",
        path: "src/test/fixtures/plan-preview.ts",
        resolvedPath: "/home/u/.tomcat/plans/src/test/fixtures/plan-preview.ts",
      },
    ]);

    fireEvent.click(await screen.findByTestId("assistant-clickable-path"));
    const intents = intentsOfType(api, "openFile") as {
      data: { line?: number; path: string };
    }[];
    expect(intents).toHaveLength(1);
    expect(intents[0].data).toEqual({
      line: 18,
      path: "/home/u/.tomcat/plans/src/test/fixtures/plan-preview.ts",
    });
  });

  it("stamps blocks with data-source-line and derives lines from the selection (even with inline markdown)", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    // The paragraph contains inline `**bold**`/`code` that the old raw-search
    // could never match; the source line now comes from the DOM attribute.
    const paragraph = document.querySelector(
      '[data-testid="plan-markdown-body"] p[data-source-line]',
    ) as HTMLElement;
    expect(paragraph).not.toBeNull();
    expect(paragraph.getAttribute("data-source-line")).toBe("12");

    mockSelectionOn(paragraph, "Some bold intro with code.");
    pushCaptureSelectionEvent();

    const intents = intentsOfType(api, "addSelectionToChat") as {
      data: { lineEnd?: number; lineStart?: number; text: string };
    }[];
    expect(intents).toHaveLength(1);
    expect(intents[0].data.text).toBe("Some bold intro with code.");
    expect(intents[0].data.lineStart).toBe(12);
    expect(intents[0].data.lineEnd).toBe(12);
  });

  it("derives distinct source lines for selections from consecutive list items", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    const items = Array.from(
      document.querySelectorAll('[data-testid="plan-markdown-body"] li[data-source-line]'),
    );
    expect(items).toHaveLength(2);
    expect(items.map((item) => item.getAttribute("data-source-line"))).toEqual(["14", "15"]);

    mockSelectionOn(items[0], "item one");
    pushCaptureSelectionEvent();
    mockSelectionOn(items[1], "item two");
    pushCaptureSelectionEvent();

    const intents = intentsOfType(api, "addSelectionToChat") as {
      data: { lineEnd?: number; lineStart?: number; text: string };
    }[];
    expect(intents.map((intent) => intent.data)).toEqual([
      { lineEnd: 14, lineStart: 14, text: "item one" },
      { lineEnd: 15, lineStart: 15, text: "item two" },
    ]);
  });

  it("omits line numbers when the selection is outside any source-mapped block", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    // The To-dos count lives outside MarkdownBody, so it has no data-source-line.
    mockSelectionOn(screen.getByTestId("plan-todos-count"), "3 To-dos");
    pushCaptureSelectionEvent();

    const intents = intentsOfType(api, "addSelectionToChat") as {
      data: { lineEnd?: number; lineStart?: number; text: string };
    }[];
    expect(intents).toHaveLength(1);
    expect(intents[0].data).not.toHaveProperty("lineStart");
    expect(intents[0].data).not.toHaveProperty("lineEnd");
  });

  it("sends nothing when the selection is empty on capture", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState());

    mockSelectionText("   ");
    pushCaptureSelectionEvent();

    expect(intentsOfType(api, "addSelectionToChat")).toHaveLength(0);
  });

  it("replaces native Find with an in-webview counter and cyclic navigation", async () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    const findState = makeState({
      bodyMarkdown: "Plan body match.\n\nAnother plan match.",
      todos: [{ content: "PLAN todo match", id: "find-todo", status: "pending" }],
    });
    pushState(findState);
    fireEvent.keyDown(document, { ctrlKey: true, key: "f" });
    const input = screen.getByTestId("plan-find-input");
    expect(
      screen
        .getByTestId("plan-content")
        .closest(".tc-plan-preview")
        ?.classList.contains("tc-plan-preview--find-open"),
    ).toBe(true);
    fireEvent.change(input, { target: { value: "plan" } });

    await waitFor(() => {
      expect(screen.getByTestId("plan-find-count").textContent).toBe("1 of 3");
    });

    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByTestId("plan-find-count").textContent).toBe("2 of 3");
    const activeHighlightBeforeHostUpdate = document.querySelector(
      ".tc-plan-find-fallback-highlight--active",
    );
    expect(activeHighlightBeforeHostUpdate).not.toBeNull();

    // A toolbar/model capability update recreates the host state object but
    // leaves searchable copy untouched; Find must retain its active result.
    pushState({ ...findState, canBuild: false });
    expect(screen.getByTestId("plan-find-count").textContent).toBe("2 of 3");
    expect(document.querySelector(".tc-plan-find-fallback-highlight--active")).toBe(
      activeHighlightBeforeHostUpdate,
    );

    // Todo status is represented by non-searchable assistive text only, so it
    // must not re-search or reset the active Find result either.
    pushState({
      ...findState,
      canBuild: false,
      todos: [{ content: "PLAN todo match", id: "find-todo", status: "completed" }],
    });    expect(screen.getByTestId("plan-find-count").textContent).toBe("2 of 3");


    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(screen.getByTestId("plan-find-count").textContent).toBe("1 of 3");

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByTestId("plan-find")).toBeNull();
  });

  it("re-searches after an asynchronous rendered-content mutation", async () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ bodyMarkdown: "Dynamic plan" }));

    fireEvent.keyDown(document, { ctrlKey: true, key: "f" });
    const input = screen.getByTestId("plan-find-input");
    expect(
      screen
        .getByTestId("plan-content")
        .closest(".tc-plan-preview")
        ?.classList.contains("tc-plan-preview--find-open"),
    ).toBe(true);
    fireEvent.change(input, { target: { value: "dynamic" } });
    await waitFor(() => {
      expect(screen.getByTestId("plan-find-count").textContent).toBe("1 of 1");
    });

    const renderedText = document.querySelector(
      ".tc-plan-find-fallback-highlight",
    )?.firstChild;
    if (!(renderedText instanceof Text)) {
      throw new Error("Expected Find's decorated text node.");
    }
    renderedText.data = "Replaced copy";

    await waitFor(() => {
      expect(screen.getByTestId("plan-find-count").textContent).toBe("No results");
    });
  });

  it("drives the in-webview Find through the host E2E DOM action protocol", async () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ bodyMarkdown: "Host PLAN_FIND_ACTION token" }));

    pushDomActionEvent({ kind: "setFindQuery", query: "plan_find_action" });

    await waitFor(() => {
      expect(screen.getByTestId("plan-find-count").textContent).toBe("1 of 1");
    });
    pushCaptureDomEvent("find-action-snapshot");

    const reply = api.postMessage.mock.calls
      .map((call) => call[0] as { data?: { findCountText?: string | null; findVisible?: boolean }; messageId?: string })
      .find((message) => message.messageId === "find-action-snapshot");
    expect(reply?.data?.findVisible).toBe(true);
    expect(reply?.data?.findCountText).toBe("1 of 1");
  });
});
