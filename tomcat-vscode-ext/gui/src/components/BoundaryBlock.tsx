import { memo } from "react";

import type { WebviewBoundaryBlock } from "../types";

function BoundaryBlockComponent({ item }: { item: WebviewBoundaryBlock }) {
  const title = item.coveredCount
    ? `Earlier history summary (${item.coveredCount} entries)`
    : "Earlier history summary";

  return (
    <details className="tc-boundary" data-testid="boundary-block">
      <summary className="tc-boundary__summary" data-testid="boundary-summary">
        {title}
      </summary>
      {item.summary ? <div className="tc-boundary__body">{item.summary}</div> : null}
    </details>
  );
}

export const BoundaryBlock = memo(
  BoundaryBlockComponent,
  (previous, next) => previous.item === next.item,
);
