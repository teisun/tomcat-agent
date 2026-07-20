export function ProgressRow({
  busy,
  hasActiveThinking,
  hasRunningTool,
  hasStreamingText,
  hasTodos,
}: {
  busy: boolean;
  hasActiveThinking: boolean;
  hasRunningTool: boolean;
  hasStreamingText: boolean;
  hasTodos: boolean;
}) {
  if (!busy || hasActiveThinking || hasRunningTool || hasStreamingText || hasTodos) {
    return null;
  }

  return (
    <div
      aria-label="Waiting for more output"
      className="tc-progress-row"
      data-testid="progress-row"
      role="status"
    >
      <span
        aria-hidden="true"
        className="tc-thinking__dots tc-progress-row__dots"
        data-testid="progress-row-dots"
      >
        ...
      </span>
    </div>
  );
}
