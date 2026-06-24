import { useEffect, useRef, useState, type RefObject } from "react";

function isAtBottom(element: HTMLElement): boolean {
  return Math.abs(element.scrollHeight - element.scrollTop - element.clientHeight) < 2;
}

type UseAutoScrollOptions = {
  containerRef: RefObject<HTMLElement | null>;
  contentKey: string;
  resetKey: string | null;
  userMessageCount: number;
};

export function useAutoScroll({
  containerRef,
  contentKey,
  resetKey,
  userMessageCount,
}: UseAutoScrollOptions) {
  const [userHasScrolled, setUserHasScrolled] = useState(false);
  const userHasScrolledRef = useRef(false);
  const previousUserMessageCountRef = useRef(userMessageCount);

  const syncUserHasScrolled = (next: boolean) => {
    userHasScrolledRef.current = next;
    setUserHasScrolled((current) => (current === next ? current : next));
  };

  const scrollToBottom = () => {
    const element = containerRef.current;
    if (!element) {
      return;
    }
    element.scrollTop = element.scrollHeight;
    syncUserHasScrolled(false);
  };

  useEffect(() => {
    syncUserHasScrolled(false);
    scrollToBottom();
  }, [resetKey]);

  useEffect(() => {
    if (userMessageCount > previousUserMessageCountRef.current) {
      scrollToBottom();
    }
    previousUserMessageCountRef.current = userMessageCount;
  }, [userMessageCount]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }

    const handleScroll = () => {
      syncUserHasScrolled(!isAtBottom(element));
    };

    element.addEventListener("scroll", handleScroll);
    return () => {
      element.removeEventListener("scroll", handleScroll);
    };
  }, [containerRef]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element || typeof ResizeObserver === "undefined") {
      return;
    }

    const observer = new ResizeObserver(() => {
      if (!userHasScrolledRef.current) {
        scrollToBottom();
      }
    });

    observer.observe(element);
    for (const child of element.children) {
      observer.observe(child);
    }

    return () => {
      observer.disconnect();
    };
  }, [containerRef, contentKey]);

  return {
    scrollToBottom,
    userHasScrolled,
  };
}
