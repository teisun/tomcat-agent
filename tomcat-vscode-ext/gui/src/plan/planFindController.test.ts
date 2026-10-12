import { afterEach, describe, expect, it, vi } from "vitest";

import { PlanFindController } from "./planFindController";

function createController(
  html: string,
  options: ConstructorParameters<typeof PlanFindController>[0] = {},
): { controller: PlanFindController; root: HTMLElement } {
  const root = document.createElement("main");
  root.innerHTML = html;
  document.body.append(root);
  const controller = new PlanFindController(options);
  controller.setSearchRoot(root);
  return { controller, root };
}

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

describe("PlanFindController", () => {
  it("publishes observable state, navigates cyclically, and clears itself on close", () => {
    const { controller } = createController("<p>Plan one</p><p>plan two</p>");
    const onChange = vi.fn();
    const unsubscribe = controller.subscribe(onChange);

    controller.open();
    controller.setQuery("plan");
    controller.refresh();
    expect(controller.getSnapshot()).toMatchObject({
      activeIndex: 0,
      open: true,
      query: "plan",
    });
    expect(controller.getSnapshot().matches).toHaveLength(2);

    controller.moveNext();
    expect(controller.getSnapshot().activeIndex).toBe(1);
    controller.moveNext();
    expect(controller.getSnapshot().activeIndex).toBe(0);
    controller.movePrev();
    expect(controller.getSnapshot().activeIndex).toBe(1);

    controller.close();
    expect(controller.getSnapshot()).toEqual({
      activeIndex: 0,
      matches: [],
      open: false,
      query: "",
    });
    expect(onChange).toHaveBeenCalled();
    unsubscribe();
  });

  it("waits for the research delay before re-searching a changed query", () => {
    vi.useFakeTimers();
    const { controller } = createController("<p>Plan one</p>", {
      researchDelayMs: 240,
    });

    controller.open();
    controller.setQuery("plan");
    expect(controller.getSnapshot().matches).toEqual([]);
    vi.advanceTimersByTime(239);
    expect(controller.getSnapshot().matches).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(controller.getSnapshot().matches).toHaveLength(1);
  });

  it("re-searches rendered content, resets/repositions the active result, and clamps it", () => {
    const { controller, root } = createController(
      "<p>plan first</p><p>plan second</p><p>plan third</p>",
    );

    controller.open();
    controller.setQuery("plan");
    controller.refresh();
    controller.moveNext();
    controller.moveNext();
    expect(controller.getSnapshot().activeIndex).toBe(2);

    // An asynchronous DOM replacement with the same logical results must keep
    // the user's selected M-of-N position instead of re-anchoring to the top.
    root.innerHTML = "<p>plan first</p><p>plan second</p><p>plan third</p>";
    controller.notifyRenderedContentChanged();
    controller.refresh();
    expect(controller.getSnapshot().activeIndex).toBe(2);

    root.innerHTML = "<p>plan only</p>";
    controller.setContentVersion("changed-plan");
    controller.refresh();
    expect(controller.getSnapshot().matches).toHaveLength(1);
    expect(controller.getSnapshot().activeIndex).toBe(0);

    controller.setQuery("only");
    controller.refresh();
    expect(controller.getSnapshot()).toMatchObject({
      activeIndex: 0,
      open: true,
      query: "only",
    });
  });

  it("caps results and leaves navigation as a no-op when there are no matches", () => {
    const { controller } = createController("<p>plan plan plan</p>", {
      matchesLimit: 2,
    });

    controller.open();
    controller.setQuery("plan");
    controller.refresh();
    expect(controller.getSnapshot().matches).toHaveLength(2);

    controller.setQuery("absent");
    controller.refresh();
    controller.moveNext();
    controller.movePrev();
    expect(controller.getSnapshot()).toMatchObject({
      activeIndex: 0,
      matches: [],
      query: "absent",
    });
  });

  it("disposes idempotently and stops publishing later mutations", () => {
    const { controller } = createController("<p>plan</p>");
    const onChange = vi.fn();
    controller.subscribe(onChange);

    controller.dispose();
    controller.dispose();
    controller.open();
    controller.setQuery("plan");
    controller.refresh();

    expect(onChange).not.toHaveBeenCalled();
    expect(controller.getSnapshot().open).toBe(false);
  });
});
