import type { WebviewCheckpoint, WebviewCheckpointMarker, WebviewTimelineItem } from "../types";

/** Scan once; both the bubble affordance and confirmation use the same write evidence. */
export function messageIdsWithWritesAfter(timeline: WebviewTimelineItem[]): Set<string> {
  const ids = new Set<string>();
  let hasWrites = false;
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const item = timeline[index];
    if (item.type === "message" && item.kind === "user" && hasWrites) ids.add(item.id);
    if (item.type === "tool" && (item.display?.kind === "file"
      || (item.display?.kind === "files" && item.display.files.some((file) => file.status === "applied")))) {
      hasWrites = true;
    }
  }
  return ids;
}

export function createCheckpointMarker(
  checkpoint: WebviewCheckpoint,
): WebviewCheckpointMarker | null {
  if (!checkpoint.messageAnchor) {
    return null;
  }
  return {
    changedFiles: [...checkpoint.changedFiles],
    checkpointId: checkpoint.id,
    createdAt: checkpoint.createdAt,
    id: `checkpoint:${checkpoint.id}`,
    kind: checkpoint.kind,
    label: checkpoint.label ?? null,
    messageAnchor: checkpoint.messageAnchor,
    type: "checkpoint",
  };
}

export function injectCheckpointMarkers(
  timeline: WebviewTimelineItem[],
  checkpoints: WebviewCheckpoint[],
): WebviewTimelineItem[] {
  const sourceTimeline = timeline.filter((item) => item.type !== "checkpoint");
  if (sourceTimeline.length === 0 || checkpoints.length === 0) {
    return sourceTimeline;
  }

  const anchorIndexById = new Map<string, number>();
  sourceTimeline.forEach((item, index) => {
    anchorIndexById.set(item.id, index);
  });

  const markersByTargetIndex = new Map<number, WebviewCheckpointMarker[]>();
  const ordered = [...checkpoints].sort((left, right) => left.createdAt.localeCompare(right.createdAt));
  for (const checkpoint of ordered) {
    if (!checkpoint.messageAnchor) {
      continue;
    }
    const anchorIndex =
      anchorIndexById.get(checkpoint.messageAnchor) ??
      anchorIndexById.get(`${checkpoint.messageAnchor}-thinking`);
    if (anchorIndex === undefined) {
      continue;
    }
    const targetIndex = sourceTimeline.findIndex(
      (item, index) => index > anchorIndex && item.type === "message" && item.kind === "user",
    );
    if (targetIndex < 0) {
      continue;
    }
    const marker = createCheckpointMarker(checkpoint);
    if (!marker) {
      continue;
    }
    const bucket = markersByTargetIndex.get(targetIndex) ?? [];
    bucket.push(marker);
    markersByTargetIndex.set(targetIndex, bucket);
  }

  if (markersByTargetIndex.size === 0) {
    return sourceTimeline;
  }

  const next: WebviewTimelineItem[] = [];
  sourceTimeline.forEach((item, index) => {
    const markers = markersByTargetIndex.get(index);
    if (markers?.length) {
      next.push(...markers);
    }
    next.push(item);
  });
  return next;
}
