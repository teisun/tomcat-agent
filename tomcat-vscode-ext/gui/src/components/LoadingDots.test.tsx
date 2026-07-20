import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LoadingDots, LOADING_DOTS_STEP_MS } from "./LoadingDots";

describe("LoadingDots", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function expectStage(expectedCount: number) {
    const dots = screen.getByTestId("loading-dots");
    expect(dots.getAttribute("data-visible-count")).toBe(String(expectedCount));
    expect(dots.querySelectorAll('[data-visible="true"]')).toHaveLength(expectedCount);
    expect(dots.querySelectorAll(".tc-loading-dots__dot")).toHaveLength(3);
  }

  it("cycles through one dot, two dots, three dots, blank, then repeats", () => {
    render(<LoadingDots testId="loading-dots" />);

    expectStage(1);

    act(() => {
      vi.advanceTimersByTime(LOADING_DOTS_STEP_MS);
    });
    expectStage(2);

    act(() => {
      vi.advanceTimersByTime(LOADING_DOTS_STEP_MS);
    });
    expectStage(3);

    act(() => {
      vi.advanceTimersByTime(LOADING_DOTS_STEP_MS);
    });
    expectStage(0);

    act(() => {
      vi.advanceTimersByTime(LOADING_DOTS_STEP_MS);
    });
    expectStage(1);
  });

  it("cleans up its interval on unmount", () => {
    const clearSpy = vi.spyOn(globalThis, "clearInterval");
    const { unmount } = render(<LoadingDots testId="loading-dots" />);

    unmount();

    expect(clearSpy).toHaveBeenCalled();
    clearSpy.mockRestore();
  });
});
