import { useEffect, useState } from "react";

export const LOADING_DOTS_STEP_MS = 500;

function nextVisibleCount(current: number): number {
  if (current >= 3) {
    return 0;
  }
  return current + 1;
}

export function LoadingDots({
  className,
  decorative = true,
  testId,
}: {
  className?: string;
  decorative?: boolean;
  testId?: string;
}) {
  const [visibleCount, setVisibleCount] = useState(1);

  useEffect(() => {
    setVisibleCount(1);
    const intervalId = globalThis.setInterval(() => {
      setVisibleCount((current) => nextVisibleCount(current));
    }, LOADING_DOTS_STEP_MS);
    return () => globalThis.clearInterval(intervalId);
  }, []);

  return (
    <span
      aria-hidden={decorative}
      className={`tc-loading-dots${className ? ` ${className}` : ""}`}
      data-testid={testId}
      data-visible-count={visibleCount}
    >
      {[0, 1, 2].map((index) => (
        <span
          className="tc-loading-dots__dot"
          data-visible={index < visibleCount ? "true" : "false"}
          key={index}
        >
          .
        </span>
      ))}
    </span>
  );
}
