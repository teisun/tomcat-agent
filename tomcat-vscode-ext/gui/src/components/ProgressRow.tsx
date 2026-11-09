import { memo } from "react";
import { useT } from "../i18n/LocaleProvider";
import type { WebviewSessionSnapshot } from "../types";

import { LoadingDots } from "./LoadingDots";

function ProgressRowComponent({
  busy,
  requestRetry,
}: {
  busy: boolean;
  requestRetry?: WebviewSessionSnapshot["requestRetry"];
}) {
  const t = useT();
  if (!busy) {
    return null;
  }

  const label = requestRetry ? t("progress.retrying", requestRetry) : t("progress.working");
  return (
    <div
      aria-label={label}
      className="tc-progress-row"
      data-testid="progress-row"
      role="status"
    >
      <LoadingDots className="tc-progress-row__dots" testId="progress-row-dots" />
      {requestRetry ? <span className="tc-progress-row__label" data-testid="progress-row-label">{label}</span> : null}
    </div>
  );
}

export const ProgressRow = memo(
  ProgressRowComponent,
  (previous, next) => previous.busy === next.busy
    && previous.requestRetry?.attempt === next.requestRetry?.attempt
    && previous.requestRetry?.maxAttempts === next.requestRetry?.maxAttempts,
);
