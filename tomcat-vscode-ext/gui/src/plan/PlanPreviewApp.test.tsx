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
    bodyMarkdown: "# Plan Heading\n\nSome **bold** intro with `code`.\n\n- item one\n- item two",
    buildModel: "",
    canBuild: true,
    mode: "preview",
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

  it("renders the mode chosen by the host (no local toggle)", () => {
    const api = makeApi();
    const { rerender } = render(<PlanPreviewApp vscodeApi={api} />);

    pushState(makeState({ mode: "markdown" }));
    expect(screen.getByTestId("plan-source")).toBeTruthy();
    expect(screen.queryByTestId("plan-markdown-body")).toBeNull();
    expect((screen.getByTestId("plan-source").textContent ?? "")).toContain("# Plan Heading");

    rerender(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ mode: "preview" }));
    expect(screen.getByTestId("plan-markdown-body")).toBeTruthy();
    expect(screen.queryByTestId("plan-source")).toBeNull();
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

  it("forwards Open in Editor to the host from the Markdown view", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ mode: "markdown" }));
    fireEvent.click(screen.getByTestId("plan-open-editor"));
    expect(intentsOfType(api, "openInTextEditor")).toHaveLength(1);
  });

  it("captures the live selection and sends addSelectionToChat with matched line numbers", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ raw: "line0\nfirst line\nsecond line\nline3" }));

    mockSelectionText("first line\nsecond line");
    pushCaptureSelectionEvent();

    const intents = intentsOfType(api, "addSelectionToChat") as {
      data: { lineEnd?: number; lineStart?: number; text: string };
    }[];
    expect(intents).toHaveLength(1);
    expect(intents[0].data.text).toBe("first line\nsecond line");
    expect(intents[0].data.lineStart).toBe(2);
    expect(intents[0].data.lineEnd).toBe(3);
  });

  it("omits line numbers when the selection cannot be located in the source", () => {
    const api = makeApi();
    render(<PlanPreviewApp vscodeApi={api} />);
    pushState(makeState({ raw: "alpha\nbeta\ngamma" }));

    mockSelectionText("text that is not in the source");
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
