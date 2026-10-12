import { afterEach, describe, expect, it, vi } from "vitest";

import type { PlanFindMatch } from "./planFindEngine";
import {
  centerPlanFindMatch,
  clearPlanFindHighlights,
  paintPlanFindHighlights,
  setPlanFindActiveHighlight,
} from "./planFindHighlight";

function makeMatch(
  text = "A plan match",
  start = 2,
  end = 6,
  parent: Node = document.body,
): PlanFindMatch {
  const node = document.createTextNode(text);
  parent.appendChild(node);
  return { segments: [{ end, node, start }] };
}

function makeSplitMatch(parent: Node = document.body): PlanFindMatch {
  const first = document.createTextNode("up");
  const second = document.createTextNode("date");
  parent.appendChild(first);
  parent.appendChild(second);
  return {
    segments: [
      { end: 2, node: first, start: 0 },
      { end: 4, node: second, start: 0 },
    ],
  };
}

function rect(top: number, bottom: number): DOMRect {
  return {
    bottom,
    height: bottom - top,
    left: 0,
    right: 100,
    toJSON: () => ({}),
    top,
    width: 100,
    x: 0,
    y: top,
  } as DOMRect;
}

const rangeRectDescriptor = Object.getOwnPropertyDescriptor(
  Range.prototype,
  "getBoundingClientRect",
);

afterEach(() => {
  clearPlanFindHighlights();
  vi.restoreAllMocks();
  if (rangeRectDescriptor) {
    Object.defineProperty(Range.prototype, "getBoundingClientRect", rangeRectDescriptor);
  } else {
    delete (Range.prototype as unknown as { getBoundingClientRect?: unknown })
      .getBoundingClientRect;
  }
  document.body.replaceChildren();
});

describe("Plan find highlights", () => {
  it("uses inline spans that stay attached to the matching text while the plan scrolls", () => {
    const container = document.createElement("main");
    document.body.append(container);
    const match = makeMatch("plan", 0, 4, container);

    paintPlanFindHighlights([match], 0);
    const highlight = container.querySelector<HTMLElement>(
      ".tc-plan-find-fallback-highlight",
    );
    expect(highlight).not.toBeNull();
    expect(highlight?.textContent).toBe("plan");
    expect(highlight?.classList.contains("tc-plan-find-fallback-highlight--active")).toBe(
      true,
    );

    container.scrollTop = 48;
    container.dispatchEvent(new Event("scroll"));

    // Find deliberately has no scroll listener or geometry layer. Browser text
    // layout moves the same inline span with its glyphs.
    expect(container.querySelector(".tc-plan-find-fallback-highlight")).toBe(highlight);
    expect(highlight?.style.position).toBe("");
  });

  it("keeps all candidates mounted and only changes the current candidate on navigation", () => {
    const first = makeMatch("first plan", 6, 10);
    const second = makeSplitMatch();

    paintPlanFindHighlights([first, second], 1);
    const candidates = Array.from(
      document.querySelectorAll<HTMLElement>(".tc-plan-find-fallback-highlight"),
    );
    expect(candidates).toHaveLength(3);
    expect(
      document.querySelectorAll(".tc-plan-find-fallback-highlight--active"),
    ).toHaveLength(2);

    setPlanFindActiveHighlight([first, second], 0);

    expect(
      Array.from(document.querySelectorAll(".tc-plan-find-fallback-highlight")),
    ).toEqual(candidates);
    expect(
      document.querySelectorAll(".tc-plan-find-fallback-highlight--active"),
    ).toHaveLength(1);
    expect(candidates[0].classList.contains("tc-plan-find-fallback-highlight--active")).toBe(
      true,
    );
  });

  it("restores the untouched source text when Find closes or the query changes", () => {
    const container = document.createElement("main");
    document.body.append(container);
    const match = makeMatch("plan", 0, 4, container);

    paintPlanFindHighlights([match], 0);
    clearPlanFindHighlights();

    expect(container.textContent).toBe("plan");
    expect(container.querySelector(".tc-plan-find-fallback-highlight")).toBeNull();
  });

  it("centres an explicitly selected match even when it is already visible", () => {
    const container = document.createElement("main");
    document.body.append(container);
    const match = makeMatch("plan", 0, 4, container);
    Object.defineProperty(container, "clientHeight", { configurable: true, value: 200 });
    Object.defineProperty(container, "scrollHeight", { configurable: true, value: 1000 });
    container.scrollTop = 75;
    vi.spyOn(container, "getBoundingClientRect").mockReturnValue(rect(0, 200));
    Object.defineProperty(Range.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => rect(100, 120),
      writable: true,
    });

    centerPlanFindMatch(match, container);

    // Match centre 110px moves to the 100px viewport centre.
    expect(container.scrollTop).toBe(85);
  });

  it("navigates with the actual decorated element instead of a stale source range", () => {
    const container = document.createElement("main");
    document.body.append(container);
    const match = makeMatch("plan", 0, 4, container);
    Object.defineProperty(container, "clientHeight", { configurable: true, value: 200 });
    Object.defineProperty(container, "scrollHeight", { configurable: true, value: 1000 });
    container.scrollTop = 100;
    vi.spyOn(container, "getBoundingClientRect").mockReturnValue(rect(0, 200));
    Object.defineProperty(Range.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => rect(-500, -480),
      writable: true,
    });

    paintPlanFindHighlights([match], 0);
    const highlight = container.querySelector<HTMLElement>(
      ".tc-plan-find-fallback-highlight",
    );
    vi.spyOn(highlight!, "getBoundingClientRect").mockReturnValue(rect(150, 170));

    centerPlanFindMatch(match, container);

    expect(container.scrollTop).toBe(160);
  });

  it("centres the selection in the readable area below fixed Find chrome", () => {
    const container = document.createElement("main");
    const find = document.createElement("section");
    find.className = "tc-plan-find";
    document.body.append(container, find);
    const match = makeMatch("plan", 0, 4, container);
    Object.defineProperty(container, "clientHeight", { configurable: true, value: 200 });
    Object.defineProperty(container, "scrollHeight", { configurable: true, value: 1000 });
    container.scrollTop = 100;
    vi.spyOn(container, "getBoundingClientRect").mockReturnValue(rect(0, 200));
    vi.spyOn(find, "getBoundingClientRect").mockReturnValue(rect(0, 40));
    Object.defineProperty(Range.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => rect(20, 40),
      writable: true,
    });

    centerPlanFindMatch(match, container);

    // Readable area is 40–200px; the 30px match centre moves to its 120px centre.
    expect(container.scrollTop).toBe(10);
  });

  it("safely skips centring when range geometry is unavailable", () => {
    const container = document.createElement("main");
    document.body.append(container);

    expect(() => centerPlanFindMatch(makeMatch("plan", 0, 4, container), container)).not.toThrow();
  });
});
