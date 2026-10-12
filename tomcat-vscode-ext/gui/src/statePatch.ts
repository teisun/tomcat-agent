import type {
  WebviewSessionPatchOp,
  WebviewStateSnapshot,
  WebviewTimelineItem,
} from "./types";

type SessionPatchApplyResult =
  | {
      ok: true;
      state: WebviewStateSnapshot;
    }
  | {
      error: string;
      ok: false;
    };

function resolveInsertIndex(
  timeline: WebviewTimelineItem[],
  op: Extract<WebviewSessionPatchOp, { type: "upsert" }>,
): number {
  if (op.beforeId) {
    const beforeIndex = timeline.findIndex((item) => item.id === op.beforeId);
    if (beforeIndex >= 0) {
      return beforeIndex;
    }
  }
  if (op.afterId) {
    const afterIndex = timeline.findIndex((item) => item.id === op.afterId);
    if (afterIndex >= 0) {
      return afterIndex + 1;
    }
  }
  return timeline.length;
}

export function applySessionPatchFrame(
  previous: WebviewStateSnapshot,
  input: {
    ops: WebviewSessionPatchOp[];
    sessionId: string;
  },
): SessionPatchApplyResult {
  const session = previous.sessionViews[input.sessionId];
  if (!session) {
    return {
      error: `missing session ${input.sessionId}`,
      ok: false,
    };
  }

  let timeline = session.timeline;
  let changed = false;

  for (const op of input.ops) {
    if (op.type === "appendText") {
      const index = timeline.findIndex((item) => item.id === op.id);
      if (index < 0) {
        return {
          error: `missing item ${op.id}`,
          ok: false,
        };
      }
      const current = timeline[index];
      if (current.type !== "message" && current.type !== "thinking") {
        return {
          error: `append target ${op.id} is not text-bearing`,
          ok: false,
        };
      }
      if (!changed) {
        timeline = [...timeline];
        changed = true;
      }
      timeline[index] = {
        ...current,
        text: `${current.text}${op.text}`,
      };
      continue;
    }

    if (op.type === "upsert") {
      if (!changed) {
        timeline = [...timeline];
        changed = true;
      }
      const existingIndex = timeline.findIndex((item) => item.id === op.item.id);
      if (existingIndex >= 0) {
        timeline.splice(existingIndex, 1);
      }
      const insertIndex = resolveInsertIndex(timeline, op);
      timeline.splice(insertIndex, 0, op.item);
      continue;
    }

    const index = timeline.findIndex((item) => item.id === op.id);
    if (index < 0) {
      return {
        error: `missing item ${op.id}`,
        ok: false,
      };
    }
    if (!changed) {
      timeline = [...timeline];
      changed = true;
    }
    timeline.splice(index, 1);
  }

  if (!changed) {
    return {
      ok: true,
      state: previous,
    };
  }

  return {
    ok: true,
    state: {
      ...previous,
      sessionViews: {
        ...previous.sessionViews,
        [input.sessionId]: {
          ...session,
          timeline,
        },
      },
    },
  };
}
