import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PlanSelectionActionButton } from "./PlanSelectionActionButton";

function mockSelection(text: string, anchorNode: Node | null): void {
  const range = {
    getBoundingClientRect: () =>
      ({ height: 16, left: 100, top: 200, width: 60 }) as DOMRect,
  };
  const selection = {
    anchorNode,
    getRangeAt: () => range,
    isCollapsed: text.length === 0,
    rangeCount: text.length === 0 ? 0 : 1,
    toString: () => text,
  };
  vi.spyOn(window, "getSelection").mockReturnValue(selection as unknown as Selection);
}

function fireSelectionChange(): void {
  act(() => {
    document.dispatchEvent(new Event("selectionchange"));
  });
}

describe("PlanSelectionActionButton", () => {
  beforeEach(() => {
    // Run rAF synchronously so selectionchange recompute is observable.
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  function setup() {
    const onAdd = vi.fn();
    render(
      <>
        <div data-testid="plan-content">
          <p id="para">hello world selection</p>
        </div>
        <PlanSelectionActionButton onAdd={onAdd} />
      </>,
    );
    const anchorNode = document.getElementById("para")?.firstChild ?? null;
    return { anchorNode, onAdd };
  }

  it("appears near a non-empty selection inside the plan content and forwards the text", () => {
    const { anchorNode, onAdd } = setup();
    mockSelection("hello world", anchorNode);
    fireSelectionChange();

    const button = screen.getByTestId("plan-selection-add");
    expect(button).toBeTruthy();
    expect(button.classList.contains("tc-plan-selection-action")).toBe(true);
    // Position is driven by inline left/top computed from the selection rect.
    expect(button.style.left).not.toBe("");
    expect(button.style.top).not.toBe("");

    fireEvent.click(button);
    expect(onAdd).toHaveBeenCalledWith("hello world");
  });

  it("stays hidden when the selection is empty", () => {
    setup();
    mockSelection("", null);
    fireSelectionChange();
    expect(screen.queryByTestId("plan-selection-add")).toBeNull();
  });

  it("stays hidden when the selection is outside the plan content", () => {
    const { onAdd } = setup();
    const outside = document.createElement("span");
    document.body.appendChild(outside);
    mockSelection("stray text", outside);
    fireSelectionChange();
    expect(screen.queryByTestId("plan-selection-add")).toBeNull();
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("hides again when the view is scrolled", () => {
    const { anchorNode } = setup();
    mockSelection("hello world", anchorNode);
    fireSelectionChange();
    expect(screen.getByTestId("plan-selection-add")).toBeTruthy();

    act(() => {
      window.dispatchEvent(new Event("scroll"));
    });
    expect(screen.queryByTestId("plan-selection-add")).toBeNull();
  });
});
