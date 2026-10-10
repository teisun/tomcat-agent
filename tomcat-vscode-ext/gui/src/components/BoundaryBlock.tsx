import { memo } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey } from "../../../src/shared/i18n";

import type { WebviewBoundaryBlock } from "../types";

function BoundaryBlockComponent({ item }: { item: WebviewBoundaryBlock }) {
  const t = useT();
  const locale = useLocale();
  const title = item.title ?? (item.coveredCount
    ? t(pluralKey(locale, "boundary.entries.other", item.coveredCount), { count: item.coveredCount })
    : t("boundary.title"));

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
