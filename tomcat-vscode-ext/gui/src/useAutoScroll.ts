import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

const BOTTOM_THRESHOLD_PX = 10;
const STICKY_PROMPT_THRESHOLD_PX = 12;

type ScrollMode = "followBottom" | "paused" | "revealUser";

export type StickyUserMetric = {
  bottom: number;
  id: string;
};

function isAtBottom(element: HTMLElement): boolean {
  return Math.abs(element.scrollHeight - element.scrollTop - element.clientHeight) < BOTTOM_THRESHOLD_PX;
}

export function selectActiveStickyUserId(
  userMetrics: StickyUserMetric[],
  scrollTop: number,
  threshold: number,
): string | null {
  let activeMetric: StickyUserMetric | null = null;
  for (const metric of userMetrics) {
    if (metric.bottom - scrollTop >= threshold) {
      continue;
    }
    if (!activeMetric || metric.bottom > activeMetric.bottom) {
      activeMetric = metric;
    }
  }
  return activeMetric?.id ?? null;
}

type UseAutoScrollOptions = {
  containerRef: RefObject<HTMLElement | null>;
  contentRef: RefObject<HTMLElement | null>;
  contentKey: string;
  lastItemIsLatestUser: boolean;
  oldestItemKey: string | null;
  resetKey: string | null;
  userMessageCount: number;
};

type LatestTurnMetrics = {
  latestTurnHeight: number;
  userBottom: number;
  userTop: number;
};

type PendingScrollAction =
  | {
      kind: "bottom";
    }
  | {
      kind: "userTop";
      top: number;
    };

function latestTurnMetrics(
  container: HTMLElement,
  content: HTMLElement,
  currentSpacerHeight: number,
): LatestTurnMetrics | null {
  const userMessages = content.querySelectorAll<HTMLElement>('[data-message-kind="user"]');
  const latestUserMessage = userMessages[userMessages.length - 1];
  if (!latestUserMessage) {
    return null;
  }

  const containerRect = container.getBoundingClientRect();
  const contentRect = content.getBoundingClientRect();
  const userRect = latestUserMessage.getBoundingClientRect();
  const userTopWithinContent = userRect.top - contentRect.top;
  const contentHeightWithoutSpacer = Math.max(0, content.scrollHeight - currentSpacerHeight);
  return {
    latestTurnHeight: Math.max(0, contentHeightWithoutSpacer - userTopWithinContent),
    userBottom: container.scrollTop + (userRect.bottom - containerRect.top),
    userTop: container.scrollTop + (userRect.top - containerRect.top),
  };
}

function currentStickyUserMetrics(
  container: HTMLElement,
  content: HTMLElement,
): StickyUserMetric[] {
  const containerRect = container.getBoundingClientRect();
  return Array.from(content.querySelectorAll<HTMLElement>('[data-message-kind="user"]'))
    .map((message) => {
      const id = message.getAttribute("data-message-id");
      if (!id) {
        return null;
      }
      const rect = message.getBoundingClientRect();
      return {
        bottom: container.scrollTop + (rect.bottom - containerRect.top),
        id,
      } satisfies StickyUserMetric;
    })
    .filter((metric): metric is StickyUserMetric => metric !== null);
}

export function useAutoScroll({
  containerRef,
  contentRef,
  contentKey,
  lastItemIsLatestUser,
  oldestItemKey,
  resetKey,
  userMessageCount,
}: UseAutoScrollOptions) {
  const [activeStickyMessageId, setActiveStickyMessageId] = useState<string | null>(null);
  const [bottomSpacerHeight, setBottomSpacerHeight] = useState(0);
  const [latestUserScrolledPast, setLatestUserScrolledPast] = useState(false);
  const [userHasScrolled, setUserHasScrolled] = useState(false);
  const autoScrollRef = useRef<{ top: number; time: number } | null>(null);
  const bottomSpacerHeightRef = useRef(0);
  const modeRef = useRef<ScrollMode>("followBottom");
  const pendingScrollActionRef = useRef<PendingScrollAction | null>(null);
  const previousOldestItemKeyRef = useRef<string | null>(oldestItemKey);
  const previousScrollHeightRef = useRef(0);
  const revealSettledRef = useRef(false);
  const skipAutoLayoutUntilRef = useRef(0);
  const userHasScrolledRef = useRef(false);
  const previousUserMessageCountRef = useRef(userMessageCount);

  const syncUserHasScrolled = (next: boolean) => {
    userHasScrolledRef.current = next;
    setUserHasScrolled((current) => (current === next ? current : next));
  };

  const syncBottomSpacerHeight = (next: number) => {
    bottomSpacerHeightRef.current = next;
    setBottomSpacerHeight((current) => (current === next ? current : next));
  };

  const syncActiveStickyMessageId = (next: string | null) => {
    setActiveStickyMessageId((current) => (current === next ? current : next));
  };

  const syncLatestUserScrolledPast = (next: boolean) => {
    setLatestUserScrolledPast((current) => (current === next ? current : next));
  };

  const markProgrammaticScroll = (top: number) => {
    autoScrollRef.current = {
      time: Date.now(),
      top,
    };
  };

  const isProgrammaticScroll = (element: HTMLElement): boolean => {
    const auto = autoScrollRef.current;
    if (!auto) {
      return false;
    }
    if (Date.now() - auto.time > 1500) {
      autoScrollRef.current = null;
      return false;
    }
    return Math.abs(element.scrollTop - auto.top) < 2;
  };

  const setScrollTop = (element: HTMLElement, top: number) => {
    const maxTop = Math.max(0, element.scrollHeight - element.clientHeight);
    const nextTop = Math.max(0, Math.min(top, maxTop));
    markProgrammaticScroll(nextTop);
    element.scrollTop = nextTop;
  };

  const updateStickyPromptState = () => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content) {
      syncActiveStickyMessageId(null);
      syncLatestUserScrolledPast(false);
      return;
    }

    const nextStickyMessageId = selectActiveStickyUserId(
      currentStickyUserMetrics(container, content),
      container.scrollTop,
      STICKY_PROMPT_THRESHOLD_PX,
    );
    syncActiveStickyMessageId(nextStickyMessageId);
    syncLatestUserScrolledPast(Boolean(nextStickyMessageId));

    const metrics = latestTurnMetrics(container, content, bottomSpacerHeightRef.current);
    if (!metrics) {
      syncLatestUserScrolledPast(false);
      return;
    }
  };

  const scrollToBottom = (queueAfterSpacer = false) => {
    const container = containerRef.current;
    if (!container) {
      return;
    }

    const hadSpacer = bottomSpacerHeightRef.current > 0;
    if (hadSpacer) {
      syncBottomSpacerHeight(0);
    }

    if (queueAfterSpacer && hadSpacer) {
      pendingScrollActionRef.current = { kind: "bottom" };
      return;
    }

    pendingScrollActionRef.current = null;
    setScrollTop(container, container.scrollHeight);
  };

  const shrinkRevealSpacer = () => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content) {
      return;
    }

    const metrics = latestTurnMetrics(container, content, bottomSpacerHeightRef.current);
    if (!metrics) {
      return;
    }

    const nextSpacerHeight = Math.max(0, container.clientHeight - metrics.latestTurnHeight);
    if (nextSpacerHeight < bottomSpacerHeightRef.current) {
      syncBottomSpacerHeight(nextSpacerHeight);
    }
  };

  const revealLatestUser = () => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content) {
      return;
    }

    const metrics = latestTurnMetrics(container, content, bottomSpacerHeightRef.current);
    if (!metrics) {
      modeRef.current = "followBottom";
      scrollToBottom(true);
      syncUserHasScrolled(false);
      return;
    }

    const nextSpacerHeight = Math.max(0, container.clientHeight - metrics.latestTurnHeight);
    const spacerChanged = nextSpacerHeight !== bottomSpacerHeightRef.current;
    syncBottomSpacerHeight(nextSpacerHeight);
    syncUserHasScrolled(false);

    if (spacerChanged) {
      revealSettledRef.current = false;
      pendingScrollActionRef.current = { kind: "userTop", top: metrics.userTop };
      return;
    }

    pendingScrollActionRef.current = null;
    setScrollTop(container, metrics.userTop);
    revealSettledRef.current = true;
  };

  const updateAutoScrollLayout = () => {
    if (Date.now() < skipAutoLayoutUntilRef.current) {
      updateStickyPromptState();
      return;
    }
    if (modeRef.current === "paused") {
      updateStickyPromptState();
      return;
    }

    if (modeRef.current === "revealUser") {
      if (pendingScrollActionRef.current?.kind === "userTop") {
        return;
      }
      if (revealSettledRef.current) {
        shrinkRevealSpacer();
        updateStickyPromptState();
        return;
      }
      revealLatestUser();
      return;
    }

    if (!userHasScrolledRef.current) {
      scrollToBottom(false);
      syncUserHasScrolled(false);
    }
  };

  const pauseFollowing = () => {
    modeRef.current = "paused";
    syncUserHasScrolled(true);
  };

  const resumeFollowing = () => {
    modeRef.current = "followBottom";
    syncUserHasScrolled(false);
    scrollToBottom(true);
  };

  const scrollToLatest = () => {
    resumeFollowing();
  };

  useLayoutEffect(() => {
    modeRef.current = "followBottom";
    pendingScrollActionRef.current = null;
    previousOldestItemKeyRef.current = oldestItemKey;
    revealSettledRef.current = false;
    syncActiveStickyMessageId(null);
    syncUserHasScrolled(false);
    syncBottomSpacerHeight(0);
    scrollToBottom(true);
    previousScrollHeightRef.current = containerRef.current?.scrollHeight ?? 0;
  }, [resetKey]);

  useLayoutEffect(() => {
    if (userMessageCount > previousUserMessageCountRef.current && lastItemIsLatestUser) {
      modeRef.current = "revealUser";
      revealSettledRef.current = false;
      syncUserHasScrolled(false);
      revealLatestUser();
    }
    previousUserMessageCountRef.current = userMessageCount;
  }, [lastItemIsLatestUser, userMessageCount]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) {
      previousOldestItemKeyRef.current = oldestItemKey;
      return;
    }
    const previousOldestItemKey = previousOldestItemKeyRef.current;
    if (
      previousOldestItemKey &&
      oldestItemKey &&
      oldestItemKey !== previousOldestItemKey
    ) {
      const delta = container.scrollHeight - previousScrollHeightRef.current;
      if (delta > 0) {
        skipAutoLayoutUntilRef.current = Date.now() + 120;
        pendingScrollActionRef.current = null;
        setScrollTop(container, container.scrollTop + delta);
        updateStickyPromptState();
      }
    }
    previousOldestItemKeyRef.current = oldestItemKey;
    previousScrollHeightRef.current = container.scrollHeight;
  }, [containerRef, oldestItemKey]);

  useLayoutEffect(() => {
    updateAutoScrollLayout();
    previousScrollHeightRef.current = containerRef.current?.scrollHeight ?? 0;
  }, [contentKey]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const pendingAction = pendingScrollActionRef.current;
    if (!container || !pendingAction) {
      updateStickyPromptState();
      return;
    }

    pendingScrollActionRef.current = null;
    if (pendingAction.kind === "bottom") {
      setScrollTop(container, container.scrollHeight);
    } else {
      setScrollTop(container, pendingAction.top);
      revealSettledRef.current = true;
    }
    updateStickyPromptState();
    previousScrollHeightRef.current = container.scrollHeight;
  }, [bottomSpacerHeight, containerRef]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }

    const handleWheel = (event: WheelEvent) => {
      if (event.deltaY >= 0) {
        return;
      }
      const target = event.target instanceof Element ? event.target : null;
      const nestedScrollable = target?.closest("[data-scrollable]");
      if (nestedScrollable && nestedScrollable !== element) {
        return;
      }
      pauseFollowing();
    };

    const handleScroll = () => {
      updateStickyPromptState();
      if (isProgrammaticScroll(element)) {
        return;
      }

      if (modeRef.current === "paused") {
        if (isAtBottom(element)) {
          modeRef.current = "followBottom";
          syncUserHasScrolled(false);
        }
        return;
      }

      if (modeRef.current === "followBottom") {
        if (!isAtBottom(element)) {
          pauseFollowing();
        }
        return;
      }

      const content = contentRef.current;
      const metrics =
        content ? latestTurnMetrics(element, content, bottomSpacerHeightRef.current) : null;
      const isPinnedToUser =
        metrics !== null && Math.abs(element.scrollTop - metrics.userTop) < 2;

      if (isPinnedToUser) {
        syncUserHasScrolled(false);
        return;
      }

      if (isAtBottom(element)) {
        modeRef.current = "followBottom";
        syncUserHasScrolled(false);
        return;
      }

      pauseFollowing();
    };

    element.addEventListener("wheel", handleWheel, { passive: true });
    element.addEventListener("scroll", handleScroll);
    return () => {
      element.removeEventListener("wheel", handleWheel);
      element.removeEventListener("scroll", handleScroll);
    };
  }, [containerRef, contentRef]);

  useEffect(() => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content || typeof ResizeObserver === "undefined") {
      return;
    }

    const observer = new ResizeObserver(() => {
      if (modeRef.current === "revealUser" && revealSettledRef.current) {
        shrinkRevealSpacer();
        updateStickyPromptState();
        return;
      }
      updateAutoScrollLayout();
      updateStickyPromptState();
    });

    observer.observe(container);
    observer.observe(content);

    return () => {
      observer.disconnect();
    };
  }, [containerRef, contentKey, contentRef]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    // Disable browser scroll anchoring while we are auto-following, otherwise
    // Electron/WebView can pin the viewport to older content and undo our
    // explicit follow-bottom positioning for the live turn.
    container.style.overflowAnchor = userHasScrolled ? "auto" : "none";
  }, [containerRef, userHasScrolled]);

  return {
    activeStickyMessageId,
    bottomSpacerHeight,
    latestUserScrolledPast,
    scrollToLatest,
    userHasScrolled,
  };
}
