import type { WebviewCheckpointMarker } from "../types";

export function CheckpointMarker({
  item,
  onRestore,
}: {
  item: WebviewCheckpointMarker;
  onRestore(checkpoint: WebviewCheckpointMarker): void;
}) {
  return (
    <div
      className="tc-checkpoint-marker"
      data-checkpoint-id={item.checkpointId}
      data-testid="checkpoint-marker"
    >
      <span aria-hidden="true" className="tc-checkpoint-marker__line" />
      <button
        className="tc-checkpoint-marker__button"
        data-testid="checkpoint-marker-button"
        onClick={() => onRestore(item)}
        type="button"
      >
        <span className="tc-checkpoint-marker__label">Restore Checkpoint</span>
        <span aria-hidden="true" className="tc-checkpoint-marker__dot">
          •
        </span>
        <span
          aria-hidden="true"
          className="codicon codicon-history tc-checkpoint-marker__icon"
        />
      </button>
      <span aria-hidden="true" className="tc-checkpoint-marker__line" />
    </div>
  );
}
