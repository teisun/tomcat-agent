import { LoadingDots } from "./LoadingDots";

export function ProgressRow({
  busy,
  hasActiveThinking,
  hasRunningTool,
  hasStreamingText,
}: {
  busy: boolean;
  hasActiveThinking: boolean;
  hasRunningTool: boolean;
  hasStreamingText: boolean;
}) {
  if (!busy || hasActiveThinking || hasRunningTool || hasStreamingText) {
    return null;
  }

  return (
    <div
      aria-label="Waiting for more output"
      className="tc-progress-row"
      data-testid="progress-row"
      role="status"
    >
      <LoadingDots className="tc-progress-row__dots" testId="progress-row-dots" />
    </div>
  );
}
