import { act, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { selectActiveStickyUserId, useAutoScroll } from "./useAutoScroll";

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
  latestUserMessageId,
  oldestItemKey = "earlier-1",
  resetKey,
  userMessageCount,
}: {
  contentKey: string;
  latestUserMessageId: string | null;
  oldestItemKey?: string | null;
  resetKey: string | null;
  userMessageCount: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const {
    activeStickyMessageId,
    bottomSpacerHeight,
    latestUserScrolledPast,
    scrollToLatest,
    userHasScrolled,
  } = useAutoScroll({
    containerRef,
    contentRef,
    contentKey,
    latestUserMessageId,
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
      <span data-testid="sticky-id">{activeStickyMessageId ?? "none"}</span>
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

  it("keeps sticky hidden while the newest user message remains visible in the viewport", () => {
    expect(selectActiveStickyUserId([], 50, 100, 12)).toBeNull();
    expect(
      selectActiveStickyUserId(
        [
          { bottom: 40, id: "user-1", top: 10 },
          { bottom: 80, id: "user-2", top: 50 },
          { bottom: 260, id: "user-3", top: 220 },
        ],
        90,
        100,
        12,
      ),
    ).toBe("user-2");
    expect(
      selectActiveStickyUserId(
        [
          { bottom: 40, id: "user-1", top: 10 },
          { bottom: 80, id: "user-2", top: 50 },
        ],
        0,
        100,
        12,
      ),
    ).toBeNull();
    expect(
      selectActiveStickyUserId(
        [
          { bottom: 30, id: "user-1", top: 0 },
          { bottom: 113, id: "user-2", top: 90 },
        ],
        100,
        100,
        12,
      ),
    ).toBeNull();
    expect(
      selectActiveStickyUserId(
        [
          { bottom: 40, id: "user-1", top: 10 },
          { bottom: 280, id: "user-2", top: 240 },
        ],
        50,
        240,
        12,
      ),
    ).toBeNull();
    expect(
      selectActiveStickyUserId(
        [
          { bottom: 40, id: "user-1", top: 10 },
          { bottom: 280, id: "user-2", top: 240 },
        ],
        50,
        120,
        12,
      ),
    ).toBe("user-1");
  });

  it("skips redundant bottom writes when follow-bottom is already pinned", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId={null}
        resetKey="s1"
        userMessageCount={0}
      />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    let scrollTop = 80;
    let scrollWrites = 0;
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
        scrollWrites += 1;
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

    rerender(
      <Fixture
        contentKey="second"
        latestUserMessageId={null}
        resetKey="s1"
        userMessageCount={0}
      />,
    );

    expect(scrollTop).toBe(80);
    expect(scrollWrites).toBe(0);
  });

  it("reveals the latest user, then resumes bottom-follow once streamed content exceeds the viewport", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId={null}
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
        latestUserMessageId="user-1"
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
        latestUserMessageId="user-1"
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    // The real browser clamps this to the new bottom once the spacer shrinks.
    // jsdom can expose either the pre-clamp write (200) or the post-clamp
    // effective bottom (160) depending on effect ordering, so accept either.
    expect([160, 200]).toContain(scrollTop);
    expect(screen.getByTestId("spacer-height").textContent).toBe("0");
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(screen.getByTestId("sticky-id").textContent).toBe("user-1");
    expect(screen.getByTestId("sticky-state").textContent).toBe("sticky");
    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");

    baseContentHeight = 280;
    rerender(
      <Fixture
        contentKey="stream-2"
        latestUserMessageId="user-1"
        resetKey="s1"
        userMessageCount={1}
      />,
    );
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(180);
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

  it("still reveals the latest user when the first thinking item lands in the same render frame", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId={null}
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
        contentKey="user-and-thinking-same-frame"
        latestUserMessageId="user-1"
        resetKey="s1"
        userMessageCount={1}
      />,
    );

    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("40");
    expect(screen.getByTestId("sticky-state").textContent).toBe("inline");

    scrollTop = 160;
    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("sticky-id").textContent).toBe("user-1");
    expect(screen.getByTestId("sticky-state").textContent).toBe("sticky");
  });

  it("does not re-reveal when older history adds user messages above the same latest turn", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId="user-1"
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

    rerender(
      <Fixture
        contentKey="prepend-older-user"
        latestUserMessageId="user-1"
        resetKey="s1"
        userMessageCount={2}
      />,
    );

    expect(scrollTop).toBe(40);
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
  });

  it("does not reveal when restore truncates the latest user turn", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId="user-2"
        resetKey="s1"
        userMessageCount={2}
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

    rerender(
      <Fixture
        contentKey="restore-truncate"
        latestUserMessageId="user-1"
        resetKey="s1"
        userMessageCount={1}
      />,
    );

    expect(scrollTop).toBe(40);
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
  });

  it("resets follow mode when the session changes", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId="user-1"
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
        latestUserMessageId="user-1"
        resetKey="s2"
        userMessageCount={1}
      />,
    );
    expect(scrollTop).toBe(80);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(root.style.overflowAnchor).toBe("none");
  });

  it("prioritizes the session reset over a reveal when a switch carries a new latest user message", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId="user-1"
        oldestItemKey="s1-oldest"
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

    // A session switch that also carries a brand-new latest user message must be
    // treated as a load (land at bottom), never as a half-applied reveal. This
    // guards the single-effect design: the reset branch is authoritative and can
    // never be clobbered/eaten by a coincident reveal trigger.
    rerender(
      <Fixture
        contentKey="switch-with-new-user"
        latestUserMessageId="user-9"
        oldestItemKey="s2-oldest"
        resetKey="s2"
        userMessageCount={3}
      />,
    );

    expect(scrollTop).toBe(80);
    expect(screen.getByTestId("spacer-height").textContent).toBe("0");
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    expect(screen.getByTestId("sticky-id").textContent).toBe("none");
    expect(root.style.overflowAnchor).toBe("none");
  });

  it("reveals a freshly sent user message even immediately after a session load", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId={null}
        oldestItemKey="s1-oldest"
        resetKey="s1"
        userMessageCount={0}
      />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    let scrollTop = 0;
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

    // Load a different session with history (webview remount / session switch):
    // must land at the bottom without revealing a pre-existing message.
    rerender(
      <Fixture
        contentKey="load-s2"
        latestUserMessageId="history-user"
        oldestItemKey="s2-oldest"
        resetKey="s2"
        userMessageCount={2}
      />,
    );
    expect(scrollTop).toBe(80);
    expect(screen.getByTestId("spacer-height").textContent).toBe("0");

    // Now send a new message in that just-loaded session: reveal must still fire
    // (the reset that just ran must not leave the trigger in a "seen" state).
    rerender(
      <Fixture
        contentKey="send-in-s2"
        latestUserMessageId="fresh-user"
        oldestItemKey="s2-oldest"
        resetKey="s2"
        userMessageCount={3}
      />,
    );
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("40");
  });

  it("re-pins the reveal when the viewport grows mid-turn (busy flip resizes the composer)", () => {
    const { rerender } = render(
      <Fixture contentKey="initial" latestUserMessageId={null} resetKey="s1" userMessageCount={0} />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    const baseContentHeight = 180;
    let clientHeight = 100;
    let scrollTop = 0;
    const currentSpacerHeight = () =>
      Number.parseFloat(screen.getByTestId("transcript-spacer").style.height || "0");

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => clientHeight,
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
      () =>
        ({
          top: 0,
          bottom: clientHeight,
          height: clientHeight,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: 0,
        }) as DOMRect,
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

    // Send a message: the reveal pins the user to the top with a spacer sized for
    // a 100px viewport (latest turn is 60px tall -> 40px spacer).
    rerender(
      <Fixture contentKey="user-1" latestUserMessageId="user-1" resetKey="s1" userMessageCount={1} />,
    );
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("40");

    // `busy` flips true -> the composer grows -> the stream viewport grows 100 -> 130
    // WITHOUT changing contentKey. The ResizeObserver must re-pin and grow the spacer
    // (100 -> 130 viewport => 70px spacer), not leave the stale 40px spacer that lets
    // the scroll clamp to the bottom and abandon the reveal.
    clientHeight = 130;
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("70");
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
  });

  it("re-pins on the resize-induced scroll clamp instead of falling into bottom-follow", () => {
    const { rerender } = render(
      <Fixture contentKey="initial" latestUserMessageId={null} resetKey="s1" userMessageCount={0} />,
    );
    const root = screen.getByTestId("scroll-root");
    const content = screen.getByTestId("scroll-content");
    const userAnchor = screen.getByTestId("user-anchor");

    const baseContentHeight = 180;
    let clientHeight = 100;
    let scrollTop = 0;
    const currentSpacerHeight = () =>
      Number.parseFloat(screen.getByTestId("transcript-spacer").style.height || "0");

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => clientHeight,
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
      () =>
        ({
          top: 0,
          bottom: clientHeight,
          height: clientHeight,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: 0,
        }) as DOMRect,
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
      <Fixture contentKey="user-1" latestUserMessageId="user-1" resetKey="s1" userMessageCount={1} />,
    );
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("40");

    // Viewport grows before the ResizeObserver re-pins: the 40px spacer is now too
    // small (content 220, viewport 130 => maxTop 90), so the browser clamps the
    // scroll from 120 to 90 and emits a scroll event. Because the latest turn still
    // fits (60 <= 130), handleScroll must re-pin, not switch to bottom-follow.
    clientHeight = 130;
    scrollTop = 90;
    act(() => {
      fireEvent.scroll(root);
    });
    expect(scrollTop).toBe(120);
    expect(screen.getByTestId("spacer-height").textContent).toBe("70");
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
  });

  it("preserves the viewport anchor when older history is prepended", () => {
    const { rerender } = render(
      <Fixture
        contentKey="initial"
        latestUserMessageId="user-1"
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
        latestUserMessageId="user-1"
        oldestItemKey="older-0"
        resetKey="s1"
        userMessageCount={1}
      />,
    );

    expect(scrollTop).toBe(140);
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");
  });
});
