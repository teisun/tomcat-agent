import { LoadingDots } from "./LoadingDots";

export function ProgressRow({
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
