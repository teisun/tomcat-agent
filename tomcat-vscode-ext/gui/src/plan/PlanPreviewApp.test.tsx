import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  PlanPreviewIntent,
  PlanPreviewStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/planPreviewProtocol";
import { PlanPreviewApp } from "./PlanPreviewApp";

function makeState(overrides: Partial<PlanPreviewStateSnapshot> = {}): PlanPreviewStateSnapshot {
  return {
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
  it("sends plan.ready on mount and shows a loading state until a frame arrives", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    expect(screen.getByTestId("plan-loading")).toBeTruthy();
    expect(intentsOfType(api, "plan.ready")).toHaveLength(1);
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
    fireEvent.change(screen.getByTestId("plan-build-model-select"), {
      target: { value: "claude-opus" },
    });
    const intents = intentsOfType(api, "setBuildModel");
    expect(intents).toHaveLength(1);
    expect((intents[0] as { data: { modelId: string } }).data.modelId).toBe("claude-opus");
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
});
