import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

const BOTTOM_THRESHOLD_PX = 10;
const STICKY_PROMPT_THRESHOLD_PX = 12;

type ScrollMode = "followBottom" | "paused" | "revealUser";

function isAtBottom(element: HTMLElement): boolean {
  return Math.abs(element.scrollHeight - element.scrollTop - element.clientHeight) < BOTTOM_THRESHOLD_PX;
}

type UseAutoScrollOptions = {
  containerRef: RefObject<HTMLElement | null>;
  contentRef: RefObject<HTMLElement | null>;
  contentKey: string;
  lastItemIsLatestUser: boolean;
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

export function useAutoScroll({
  containerRef,
  contentRef,
  contentKey,
  lastItemIsLatestUser,
  resetKey,
  userMessageCount,
}: UseAutoScrollOptions) {
  const [bottomSpacerHeight, setBottomSpacerHeight] = useState(0);
  const [latestUserScrolledPast, setLatestUserScrolledPast] = useState(false);
  const [userHasScrolled, setUserHasScrolled] = useState(false);
  const autoScrollRef = useRef<{ top: number; time: number } | null>(null);
  const bottomSpacerHeightRef = useRef(0);
  const modeRef = useRef<ScrollMode>("followBottom");
  const pendingScrollActionRef = useRef<PendingScrollAction | null>(null);
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
      syncLatestUserScrolledPast(false);
      return;
    }

    const metrics = latestTurnMetrics(container, content, bottomSpacerHeightRef.current);
    if (!metrics) {
      syncLatestUserScrolledPast(false);
      return;
    }

    syncLatestUserScrolledPast(
      metrics.userBottom - container.scrollTop < STICKY_PROMPT_THRESHOLD_PX,
    );
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

    if (metrics.latestTurnHeight > container.clientHeight) {
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
      pendingScrollActionRef.current = { kind: "userTop", top: metrics.userTop };
      return;
    }

    pendingScrollActionRef.current = null;
    setScrollTop(container, metrics.userTop);
  };

  const updateAutoScrollLayout = () => {
    if (modeRef.current === "paused") {
      updateStickyPromptState();
      return;
    }

    if (modeRef.current === "revealUser") {
      if (pendingScrollActionRef.current?.kind === "userTop") {
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
    syncUserHasScrolled(false);
    syncBottomSpacerHeight(0);
    scrollToBottom(true);
  }, [resetKey]);

  useLayoutEffect(() => {
    if (userMessageCount > previousUserMessageCountRef.current && lastItemIsLatestUser) {
      modeRef.current = "revealUser";
      syncUserHasScrolled(false);
      revealLatestUser();
    }
    previousUserMessageCountRef.current = userMessageCount;
  }, [lastItemIsLatestUser, userMessageCount]);

  useLayoutEffect(() => {
    updateAutoScrollLayout();
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
    }
    updateStickyPromptState();
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
    bottomSpacerHeight,
    latestUserScrolledPast,
    scrollToLatest,
    userHasScrolled,
  };
}
