import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WebviewToolCard } from "../types";
import { GroupActivityTicker } from "./GroupActivityTicker";

const MIN_DWELL_MS = 450;
const ROLL_MS = 260;
const COLLAPSE_MS = 160;

function buildTool(
  id: string,
  status: WebviewToolCard["status"],
  overrides: Partial<WebviewToolCard> = {},
): WebviewToolCard {
  return {
    args: { path: `/workspace/${id}.ts` },
    assistantMessageId: "assistant-1",
    display: { file: `/workspace/${id}.ts`, kind: "file" },
    id,
    isError: false,
    status,
    summary: `summary for ${id}`,
    toolCallId: `tc-${id}`,
    toolName: "read",
    type: "tool",
    ...overrides,
  };
}

describe("GroupActivityTicker", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("renders nothing for inactive history groups", () => {
    render(
      <GroupActivityTicker
        isLive={false}
        tools={[buildTool("readme", "complete")]}
      />,
    );

    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
  });

  it("shows the current running tool with a shimmering label", () => {
    render(
      <GroupActivityTicker
        isLive
        tools={[buildTool("active", "running")]}
      />,
    );

    const label = screen.getByText("Reading file active.ts");
    expect(label.className).toContain("tc-loading-shimmer");
    expect(screen.getByTestId("group-activity-ticker")).toBeTruthy();
  });

  it("starts collapsed and clears its entering class on the next tick", () => {
    vi.useFakeTimers();
    render(
      <GroupActivityTicker
        isLive
        tools={[buildTool("active", "running")]}
      />,
    );

    expect(screen.getByTestId("group-activity-ticker").className).toContain(
      "tc-group-ticker--entering",
    );

    act(() => {
      vi.runOnlyPendingTimers();
    });

    expect(screen.getByTestId("group-activity-ticker").className).not.toContain(
      "tc-group-ticker--entering",
    );
  });

  it("skips the entering class when reduced motion is enabled", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn().mockImplementation(() => ({
        addEventListener: vi.fn(),
        addListener: vi.fn(),
        dispatchEvent: vi.fn(),
        matches: true,
        media: "(prefers-reduced-motion: reduce)",
        onchange: null,
        removeEventListener: vi.fn(),
        removeListener: vi.fn(),
      })),
    );

    render(
      <GroupActivityTicker
        isLive
        tools={[buildTool("active", "running")]}
      />,
    );

    expect(screen.getByTestId("group-activity-ticker").className).not.toContain(
      "tc-group-ticker--entering",
    );
  });

  it("rolls through queued tools one by one after each completion", () => {
    vi.useFakeTimers();
    const firstRunning = buildTool("first", "running");
    const firstComplete = buildTool("first", "complete");
    const secondRunning = buildTool("second", "running");
    const secondComplete = buildTool("second", "complete");
    const thirdRunning = buildTool("third", "running");
    const { container, rerender } = render(
      <GroupActivityTicker
        isLive
        tools={[firstRunning]}
      />,
    );

    rerender(
      <GroupActivityTicker
        isLive
        tools={[firstComplete, secondRunning]}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(MIN_DWELL_MS);
    });
    expect(container.querySelector(".tc-group-ticker__strip--rolling")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(ROLL_MS);
    });
    expect(screen.getByText("Reading file second.ts")).toBeTruthy();

    rerender(
      <GroupActivityTicker
        isLive
        tools={[firstComplete, secondComplete, thirdRunning]}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(MIN_DWELL_MS);
    });
    expect(container.querySelector(".tc-group-ticker__strip--rolling")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(ROLL_MS);
    });
    expect(screen.getByText("Reading file third.ts")).toBeTruthy();
  });

  it("lingers on the last completed tool while the live turn is still active", () => {
    vi.useFakeTimers();
    const firstComplete = buildTool("first", "complete");
    const secondRunning = buildTool("second", "running");
    const { container, rerender } = render(
      <GroupActivityTicker
        isLive
        tools={[firstComplete]}
      />,
    );

    const lingeringLabel = screen.getByText("Read file first.ts");
    expect(lingeringLabel.className).toContain("tc-loading-shimmer");

    rerender(
      <GroupActivityTicker
        isLive
        tools={[firstComplete, secondRunning]}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(MIN_DWELL_MS);
    });
    expect(container.querySelector(".tc-group-ticker__strip--rolling")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(ROLL_MS);
    });
    expect(screen.getByText("Reading file second.ts")).toBeTruthy();
  });

  it("exits in two phases when the group stops being live", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <GroupActivityTicker
        isLive
        tools={[buildTool("done", "complete")]}
      />,
    );

    rerender(
      <GroupActivityTicker
        isLive={false}
        tools={[buildTool("done", "complete")]}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(MIN_DWELL_MS);
    });
    expect(container.querySelector(".tc-group-ticker__strip--rolling")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(ROLL_MS);
    });
    expect(container.querySelector(".tc-group-ticker--collapsing")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(COLLAPSE_MS);
    });
    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
  });

  it("still finalizes its exit when reduced motion disables transitions", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "matchMedia",
      vi.fn().mockImplementation(() => ({
        addEventListener: vi.fn(),
        addListener: vi.fn(),
        dispatchEvent: vi.fn(),
        matches: true,
        media: "(prefers-reduced-motion: reduce)",
        onchange: null,
        removeEventListener: vi.fn(),
        removeListener: vi.fn(),
      })),
    );
    const { rerender } = render(
      <GroupActivityTicker
        isLive
        tools={[buildTool("done", "complete")]}
      />,
    );

    rerender(
      <GroupActivityTicker
        isLive={false}
        tools={[buildTool("done", "complete")]}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(MIN_DWELL_MS);
      vi.runAllTimers();
    });
    expect(screen.queryByTestId("group-activity-ticker")).toBeNull();
  });
});
