import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { WebviewToolCard } from "../types";
import { buildGroupTitleFromTool, isRunning, toolIconClass } from "./ToolRow";

const MIN_DWELL_MS = 450;
const ROLL_MS = 260;
const COLLAPSE_MS = 160;

type GroupActivityTickerProps = {
  isLive: boolean;
  tools: WebviewToolCard[];
};

type TickerSlot = {
  index: number | null;
  key: string;
};

type TimerRef = {
  current: number | null;
};

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

function initialTickerIndex(tools: WebviewToolCard[]): number | null {
  if (tools.length === 0) {
    return null;
  }
  const firstRunningIndex = tools.findIndex((tool) => isRunning(tool));
  return firstRunningIndex >= 0 ? firstRunningIndex : tools.length - 1;
}

function toolSlot(tools: WebviewToolCard[], index: number): TickerSlot {
  const tool = tools[index];
  return {
    index,
    key: tool ? `tool-${tool.id}` : `tool-index-${index}`,
  };
}

function emptySlot(slotId: string): TickerSlot {
  return {
    index: null,
    key: `empty-${slotId}`,
  };
}

export function GroupActivityTicker({
  isLive,
  tools,
}: GroupActivityTickerProps) {
  const reducedMotion = useMemo(() => prefersReducedMotion(), []);
  const rollMs = reducedMotion ? 0 : ROLL_MS;
  const collapseMs = reducedMotion ? 0 : COLLAPSE_MS;
  const initialIndex = isLive ? initialTickerIndex(tools) : null;
  const [currentIndex, setCurrentIndex] = useState<number | null>(initialIndex);
  const [slots, setSlots] = useState<TickerSlot[]>(
    initialIndex === null ? [] : [toolSlot(tools, initialIndex)],
  );
  const [rolling, setRolling] = useState(false);
  const [exiting, setExiting] = useState(false);
  const [entering, setEntering] = useState(false);
  const [finished, setFinished] = useState(false);
  const startedRef = useRef(isLive);
  const shownAtRef = useRef(Date.now());
  const wasVisibleRef = useRef(false);
  const enterTimerRef = useRef<number | null>(null);
  const rollTimerRef = useRef<number | null>(null);
  const collapseTimerRef = useRef<number | null>(null);

  const clearTimer = useCallback((timerRef: TimerRef) => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const clearTimers = useCallback(() => {
    clearTimer(enterTimerRef);
    clearTimer(rollTimerRef);
    clearTimer(collapseTimerRef);
  }, [clearTimer]);

  useEffect(() => {
    return () => {
      clearTimers();
    };
  }, [clearTimers]);

  useEffect(() => {
    if (isLive) {
      startedRef.current = true;
    }
  }, [isLive]);

  useEffect(() => {
    if (!isLive && !startedRef.current) {
      return;
    }
    if (tools.length === 0) {
      setCurrentIndex(null);
      setSlots([]);
      return;
    }
    setCurrentIndex((previousIndex) => {
      if (previousIndex === null) {
        return initialTickerIndex(tools);
      }
      return Math.min(previousIndex, tools.length - 1);
    });
  }, [isLive, tools]);

  useEffect(() => {
    if (currentIndex === null) {
      return;
    }
    shownAtRef.current = Date.now();
  }, [currentIndex]);

  useLayoutEffect(() => {
    const visible = slots.length > 0 && !finished;
    if (!visible) {
      wasVisibleRef.current = false;
      setEntering(false);
      clearTimer(enterTimerRef);
      return;
    }

    if (!wasVisibleRef.current && !reducedMotion) {
      setEntering(true);
    }
    wasVisibleRef.current = true;
  }, [clearTimer, finished, reducedMotion, slots.length]);

  useEffect(() => {
    if (!entering) {
      return;
    }

    clearTimer(enterTimerRef);
    enterTimerRef.current = window.setTimeout(() => {
      setEntering(false);
      enterTimerRef.current = null;
    }, 0);

    return () => {
      clearTimer(enterTimerRef);
    };
  }, [clearTimer, entering]);

  useEffect(() => {
    if (currentIndex === null || rolling || exiting || finished) {
      return;
    }
    setSlots((previousSlots) => {
      const nextSlot = toolSlot(tools, currentIndex);
      if (
        previousSlots.length === 1 &&
        previousSlots[0]?.index === currentIndex &&
        previousSlots[0]?.key === nextSlot.key
      ) {
        return previousSlots;
      }
      return [nextSlot];
    });
  }, [currentIndex, exiting, finished, rolling, tools]);

  const startRollToIndex = useCallback((nextIndex: number) => {
    if (rolling || exiting || currentIndex === null || !tools[nextIndex]) {
      return;
    }
    clearTimer(rollTimerRef);
    setSlots([toolSlot(tools, currentIndex), toolSlot(tools, nextIndex)]);
    setRolling(true);
    rollTimerRef.current = window.setTimeout(() => {
      setRolling(false);
      setCurrentIndex(nextIndex);
      setSlots([toolSlot(tools, nextIndex)]);
      rollTimerRef.current = null;
    }, rollMs);
  }, [clearTimer, currentIndex, exiting, rollMs, rolling, tools]);

  const startExit = useCallback(() => {
    if (rolling || exiting || finished || currentIndex === null) {
      return;
    }
    const sourceTool = tools[currentIndex];
    const exitKey = sourceTool ? sourceTool.id : `${currentIndex}`;
    clearTimers();
    setSlots([toolSlot(tools, currentIndex), emptySlot(exitKey)]);
    setRolling(true);
    rollTimerRef.current = window.setTimeout(() => {
      setRolling(false);
      setExiting(true);
      setSlots([emptySlot(exitKey)]);
      collapseTimerRef.current = window.setTimeout(() => {
        setExiting(false);
        setFinished(true);
        setSlots([]);
        collapseTimerRef.current = null;
      }, collapseMs);
      rollTimerRef.current = null;
    }, rollMs);
  }, [clearTimers, collapseMs, currentIndex, exiting, finished, rollMs, rolling, tools]);

  useEffect(() => {
    if (!startedRef.current || finished || rolling || exiting || currentIndex === null) {
      return;
    }
    const currentTool = tools[currentIndex];
    if (!currentTool) {
      return;
    }

    const dwellRemaining = Math.max(0, MIN_DWELL_MS - (Date.now() - shownAtRef.current));
    const hasNextTool = currentIndex + 1 < tools.length;

    if (!isLive) {
      const timerId = window.setTimeout(() => {
        startExit();
      }, dwellRemaining);
      return () => {
        window.clearTimeout(timerId);
      };
    }

    if (!isRunning(currentTool) && hasNextTool) {
      const timerId = window.setTimeout(() => {
        startRollToIndex(currentIndex + 1);
      }, dwellRemaining);
      return () => {
        window.clearTimeout(timerId);
      };
    }
  }, [currentIndex, exiting, finished, isLive, rolling, startExit, startRollToIndex, tools]);

  if ((!startedRef.current && !isLive) || finished || slots.length === 0) {
    return null;
  }

  const currentTool =
    currentIndex === null || currentIndex >= tools.length ? null : tools[currentIndex];
  const lingerOnLastTool =
    isLive &&
    !rolling &&
    !exiting &&
    currentTool !== null &&
    currentIndex === tools.length - 1 &&
    !isRunning(currentTool);

  return (
    <div
      aria-hidden="true"
      className={`tc-group-ticker${entering ? " tc-group-ticker--entering" : ""}${exiting ? " tc-group-ticker--collapsing" : ""}`}
      data-testid="group-activity-ticker"
    >
      <div
        className={`tc-group-ticker__strip${rolling ? " tc-group-ticker__strip--rolling" : ""}`}
      >
        {slots.map((slot) => {
          if (slot.index === null) {
            return <span className="tc-group-ticker__line" key={slot.key} />;
          }
          const tool = tools[slot.index];
          if (!tool) {
            return null;
          }
          const shouldShimmer =
            isRunning(tool) || (lingerOnLastTool && currentTool?.id === tool.id);
          return (
            <span
              className="tc-group-ticker__line"
              data-testid="group-activity-ticker-line"
              key={slot.key}
            >
              <span
                aria-hidden="true"
                className={`tc-group-ticker__icon codicon ${toolIconClass(tool.toolName)}`}
              />
              <span
                className={`tc-group-ticker__label${shouldShimmer ? " tc-loading-shimmer" : ""}`}
              >
                {buildGroupTitleFromTool(tool)}
              </span>
            </span>
          );
        })}
      </div>
    </div>
  );
}
