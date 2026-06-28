import { act, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAutoScroll } from "./useAutoScroll";

class ResizeObserverMock {
  static instances: ResizeObserverMock[] = [];

  readonly callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ResizeObserverMock.instances.push(this);
  }

  disconnect() {}

  observe() {}

  static latest(): ResizeObserverMock {
    const latest = ResizeObserverMock.instances.at(-1);
    if (!latest) {
      throw new Error("ResizeObserver was not instantiated");
    }
    return latest;
  }

  static reset() {
    ResizeObserverMock.instances = [];
  }
}

function Fixture({
  contentKey,
  lastItemIsLatestUser,
  oldestItemKey = "earlier-1",
  resetKey,
  userMessageCount,
}: {
  contentKey: string;
  lastItemIsLatestUser: boolean;
  oldestItemKey?: string | null;
  resetKey: string | null;
  userMessageCount: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const { bottomSpacerHeight, latestUserScrolledPast, scrollToLatest, userHasScrolled } = useAutoScroll({
    containerRef,
    contentRef,
    contentKey,
    lastItemIsLatestUser,
    oldestItemKey,
    resetKey,
    userMessageCount,
  });

  return (
    <>
      <div ref={containerRef} data-testid="scroll-root">
        <div ref={contentRef} data-testid="scroll-content">
          <div data-message-kind="assistant">earlier</div>
          <div data-message-id="user-1" data-message-kind="user" data-testid="user-anchor">
            user
          </div>
          <div data-message-kind="assistant">latest turn</div>
          <div
            aria-hidden="true"
            data-testid="transcript-spacer"
            style={{ height: `${bottomSpacerHeight}px` }}
          />
        </div>
      </div>
      <span data-testid="spacer-height">{bottomSpacerHeight}</span>
      <span data-testid="scroll-state">{userHasScrolled ? "paused" : "following"}</span>
      <span data-testid="sticky-state">{latestUserScrolledPast ? "sticky" : "inline"}</span>
      <button onClick={scrollToLatest} type="button">
        Jump
      </button>
    </>
  );
}

describe("useAutoScroll", () => {
  beforeEach(() => {
    ResizeObserverMock.reset();
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reveals the latest user once and keeps it pinned while streamed content grows", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        lastItemIsLatestUser={false}
        resetKey="s1"
        userMessageCount={0}
      />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    let baseContentHeight = 180;
    let scrollTop = 0;
    const currentSpacerHeight = () =>
      Number.parseFloat(screen.getByTestId("transcript-spacer").style.height || "0");

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      get: () => baseContentHeight + currentSpacerHeight(),
    });
    Object.defineProperty(root, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    Object.defineProperty(content, "scrollHeight", {
      configurable: true,
      get: () => baseContentHeight + currentSpacerHeight(),
    });
    root.getBoundingClientRect = vi.fn(
      () => ({ top: 0, bottom: 100, height: 100, left: 0, right: 0, width: 0, x: 0, y: 0 }) as DOMRect,
    );
    content.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: -scrollTop,
          bottom: baseContentHeight - scrollTop,
          height: baseContentHeight,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: -scrollTop,
        }) as DOMRect,
    );
    userAnchor.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: 120 - scrollTop,
          bottom: 150 - scrollTop,
          height: 30,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: 120 - scrollTop,
        }) as DOMRect,
    );

    rerender(
      <Fixture
        contentKey="user-1"
        lastItemIsLatestUser={true}
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("40");
    expect(screen.getByTestId("sticky-state").textContent).toBe("inline");

    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(root.style.overflowAnchor).toBe("none");

    baseContentHeight = 260;
    rerender(
      <Fixture
        contentKey="stream-1"
        lastItemIsLatestUser={false}
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("0");
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(screen.getByTestId("sticky-state").textContent).toBe("inline");
    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");

    baseContentHeight = 280;
    rerender(
      <Fixture
        contentKey="stream-2"
        lastItemIsLatestUser={false}
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("0");

    scrollTop = 60;
    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
    expect(root.style.overflowAnchor).toBe("auto");

    baseContentHeight = 320;
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(60);

    act(() => {
      fireEvent.click(screen.getByText("Jump"));
    });
    expect(scrollTop).toBe(220);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(root.style.overflowAnchor).toBe("none");
  });

  it("resets follow mode when the session changes", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        lastItemIsLatestUser={false}
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    let scrollTop = 25;
    const currentSpacerHeight = () =>
      Number.parseFloat(screen.getByTestId("transcript-spacer").style.height || "0");

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      get: () => 180 + currentSpacerHeight(),
    });
    Object.defineProperty(root, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    Object.defineProperty(content, "scrollHeight", {
      configurable: true,
      get: () => 180 + currentSpacerHeight(),
    });
    root.getBoundingClientRect = vi.fn(
      () => ({ top: 0, bottom: 100, height: 100, left: 0, right: 0, width: 0, x: 0, y: 0 }) as DOMRect,
    );
    content.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: -scrollTop,
          bottom: 180 - scrollTop,
          height: 180,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: -scrollTop,
        }) as DOMRect,
    );
    userAnchor.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: 120 - scrollTop,
          bottom: 150 - scrollTop,
          height: 30,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: 120 - scrollTop,
        }) as DOMRect,
    );

    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
    expect(root.style.overflowAnchor).toBe("auto");

    rerender(
      <Fixture
        contentKey="initial"
        lastItemIsLatestUser={false}
        resetKey="s2"
        userMessageCount={1}
      />,
    );
    expect(scrollTop).toBe(80);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(root.style.overflowAnchor).toBe("none");
  });

  it("preserves the viewport anchor when older history is prepended", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        lastItemIsLatestUser={false}
        oldestItemKey="older-1"
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    let baseContentHeight = 180;
    let scrollTop = 40;
    const currentSpacerHeight = () =>
      Number.parseFloat(screen.getByTestId("transcript-spacer").style.height || "0");

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      get: () => baseContentHeight + currentSpacerHeight(),
    });
    Object.defineProperty(root, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    Object.defineProperty(content, "scrollHeight", {
      configurable: true,
      get: () => baseContentHeight + currentSpacerHeight(),
    });
    root.getBoundingClientRect = vi.fn(
      () => ({ top: 0, bottom: 100, height: 100, left: 0, right: 0, width: 0, x: 0, y: 0 }) as DOMRect,
    );
    content.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: -scrollTop,
          bottom: baseContentHeight - scrollTop,
          height: baseContentHeight,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: -scrollTop,
        }) as DOMRect,
    );
    userAnchor.getBoundingClientRect = vi.fn(
      () =>
        ({
          top: 120 - scrollTop,
          bottom: 150 - scrollTop,
          height: 30,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: 120 - scrollTop,
        }) as DOMRect,
    );

    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");

    baseContentHeight = 240;
    rerender(
      <Fixture
        contentKey="prepended"
        lastItemIsLatestUser={false}
        oldestItemKey="older-0"
        resetKey="s1"
        userMessageCount={1}
      />,
    );

    expect(scrollTop).toBe(140);
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
  });
});
