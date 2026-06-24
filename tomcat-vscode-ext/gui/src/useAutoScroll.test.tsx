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
  resetKey,
  userMessageCount,
}: {
  contentKey: string;
  resetKey: string | null;
  userMessageCount: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const { scrollToBottom, userHasScrolled } = useAutoScroll({
    containerRef,
    contentKey,
    resetKey,
    userMessageCount,
  });

  return (
    <>
      <div ref={containerRef} data-testid="scroll-root">
        <div data-testid="scroll-content">content</div>
      </div>
      <span data-testid="scroll-state">{userHasScrolled ? "paused" : "following"}</span>
      <button onClick={scrollToBottom} type="button">
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

  it("follows content growth until the user scrolls away, then resets on the next user message", () => {
    const { rerender } = render(
      <Fixture contentKey="initial" resetKey="s1" userMessageCount={0} />,
    );
    const root = screen.getByTestId("scroll-root");

    let scrollHeight = 300;
    let scrollTop = 200;

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      get: () => scrollHeight,
    });
    Object.defineProperty(root, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });

    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");

    scrollHeight = 420;
    rerender(<Fixture contentKey="stream-1" resetKey="s1" userMessageCount={0} />);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(420);

    scrollTop = 40;
    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");

    scrollHeight = 520;
    act(() => {
      ResizeObserverMock.latest().callback([], {} as ResizeObserver);
    });
    expect(scrollTop).toBe(40);

    rerender(<Fixture contentKey="initial" resetKey="s1" userMessageCount={1} />);
    expect(scrollTop).toBe(520);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
  });

  it("resets follow mode when the session changes", () => {
    const { rerender } = render(
      <Fixture contentKey="initial" resetKey="s1" userMessageCount={0} />,
    );
    const root = screen.getByTestId("scroll-root");

    let scrollTop = 25;

    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      get: () => 300,
    });
    Object.defineProperty(root, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });

    act(() => {
      fireEvent.scroll(root);
    });
    expect(screen.getByTestId("scroll-state").textContent).toBe("paused");

    rerender(<Fixture contentKey="initial" resetKey="s2" userMessageCount={0} />);
    expect(scrollTop).toBe(300);
    expect(screen.getByTestId("scroll-state").textContent).toBe("following");
  });
});
