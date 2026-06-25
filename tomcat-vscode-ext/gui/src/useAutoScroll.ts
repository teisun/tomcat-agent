import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

function isAtBottom(element: HTMLElement): boolean {
  return Math.abs(element.scrollHeight - element.scrollTop - element.clientHeight) < 2;
}

type UseAutoScrollOptions = {
  containerRef: RefObject<HTMLElement | null>;
  contentRef: RefObject<HTMLElement | null>;
  contentKey: string;
  resetKey: string | null;
  userMessageCount: number;
};

type LatestTurnMetrics = {
  latestTurnHeight: number;
  userTop: number;
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
    userTop: container.scrollTop + (userRect.top - containerRect.top),
  };
}

export function useAutoScroll({
  containerRef,
  contentRef,
  contentKey,
  resetKey,
  userMessageCount,
}: UseAutoScrollOptions) {
  const [bottomSpacerHeight, setBottomSpacerHeight] = useState(0);
  const [userHasScrolled, setUserHasScrolled] = useState(false);
  const anchorTopRef = useRef<number | null>(null);
  const autoScrollRef = useRef<{ top: number; time: number } | null>(null);
  const bottomSpacerHeightRef = useRef(0);
  const pendingScrollTopRef = useRef<number | null>(null);
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

  const updateFollowLayout = (shouldAutoScroll: boolean) => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container) {
      return;
    }

    const metrics =
      content ? latestTurnMetrics(container, content, bottomSpacerHeightRef.current) : null;
    anchorTopRef.current = metrics?.userTop ?? null;
    const previousSpacerHeight = bottomSpacerHeightRef.current;

    if (!metrics) {
      pendingScrollTopRef.current = null;
      syncBottomSpacerHeight(0);
      if (shouldAutoScroll) {
        setScrollTop(container, container.scrollHeight);
        syncUserHasScrolled(false);
      }
      return;
    }

    const nextSpacerHeight = Math.max(0, container.clientHeight - metrics.latestTurnHeight);
    syncBottomSpacerHeight(nextSpacerHeight);

    if (!shouldAutoScroll) {
      return;
    }

    anchorTopRef.current = metrics.userTop;
    if (nextSpacerHeight !== previousSpacerHeight) {
      pendingScrollTopRef.current = metrics.userTop;
    } else {
      pendingScrollTopRef.current = null;
      setScrollTop(container, metrics.userTop);
    }
    syncUserHasScrolled(false);
  };

  const scrollToLatest = () => {
    syncUserHasScrolled(false);
    updateFollowLayout(true);
  };

  useLayoutEffect(() => {
    anchorTopRef.current = null;
    syncBottomSpacerHeight(0);
    scrollToLatest();
  }, [resetKey]);

  useLayoutEffect(() => {
    if (userMessageCount > previousUserMessageCountRef.current) {
      scrollToLatest();
    }
    previousUserMessageCountRef.current = userMessageCount;
  }, [userMessageCount]);

  useLayoutEffect(() => {
    updateFollowLayout(!userHasScrolledRef.current);
  }, [contentKey]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const pendingScrollTop = pendingScrollTopRef.current;
    if (!container || pendingScrollTop === null) {
      return;
    }
    pendingScrollTopRef.current = null;
    setScrollTop(container, pendingScrollTop);
  }, [bottomSpacerHeight, containerRef]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }

    const handleScroll = () => {
      if (isProgrammaticScroll(element)) {
        return;
      }
      const isPinnedToUser =
        anchorTopRef.current !== null && Math.abs(element.scrollTop - anchorTopRef.current) < 2;
      syncUserHasScrolled(!(isPinnedToUser || isAtBottom(element)));
    };

    element.addEventListener("scroll", handleScroll);
    return () => {
      element.removeEventListener("scroll", handleScroll);
    };
  }, [containerRef]);

  useEffect(() => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content || typeof ResizeObserver === "undefined") {
      return;
    }

    const observer = new ResizeObserver(() => {
      updateFollowLayout(!userHasScrolledRef.current);
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
    // explicit "latest user at top" positioning.
    container.style.overflowAnchor = userHasScrolled ? "auto" : "none";
  }, [containerRef, userHasScrolled]);

  return {
    bottomSpacerHeight,
    scrollToLatest,
    userHasScrolled,
  };
}
