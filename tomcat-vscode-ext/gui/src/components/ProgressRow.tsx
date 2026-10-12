import { memo } from "react";
import { useT } from "../i18n/LocaleProvider";

import { LoadingDots } from "./LoadingDots";

function ProgressRowComponent({
  busy,
}: {
  busy: boolean;
}) {
  const t = useT();
  if (!busy) {
    return null;
  }

  return (
    <div
      aria-label={t("progress.working")}
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
