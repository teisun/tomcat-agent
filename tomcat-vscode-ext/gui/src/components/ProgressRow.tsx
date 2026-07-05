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
    <div className="tc-progress-row tc-progress-row--shimmer" data-testid="progress-row">
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
