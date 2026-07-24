import { memo } from "react";

import { LoadingDots } from "./LoadingDots";

function ProgressRowComponent({
  busy,
}: {
  busy: boolean;
}) {
  if (!busy) {
    return null;
  }

  return (
    <div
      aria-label="Still working"
      className="tc-progress-row"
      data-testid="progress-row"
      role="status"
    >
      <LoadingDots className="tc-progress-row__dots" testId="progress-row-dots" />
    </div>
  );
}

export const ProgressRow = memo(
  ProgressRowComponent,
  (previous, next) => previous.busy === next.busy,
);
