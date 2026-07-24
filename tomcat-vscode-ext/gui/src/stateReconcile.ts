import type {
  WebviewCheckpoint,
  WebviewPendingAttachment,
  WebviewSessionSnapshot,
  WebviewSessionTab,
  WebviewStateSnapshot,
  WebviewTimelineItem,
  WebviewTodo,
} from "./types";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function deepEqual(left: unknown, right: unknown): boolean {
  if (left === right) {
    return true;
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) {
      return false;
    }
    return left.every((entry, index) => deepEqual(entry, right[index]));
  }
  if (!isRecord(left) || !isRecord(right)) {
    return false;
  }
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  if (leftKeys.length !== rightKeys.length) {
    return false;
  }
  return leftKeys.every((key) => key in right && deepEqual(left[key], right[key]));
}

function reconcileValue<T>(previous: T | undefined, next: T): T {
  return previous !== undefined && deepEqual(previous, next) ? previous : next;
}

function reconcilePrimitiveArray<T extends string | number | boolean>(
  previous: readonly T[] | undefined,
  next: readonly T[],
): T[] {
  if (
    previous &&
    previous.length === next.length &&
    previous.every((entry, index) => entry === next[index])
  ) {
    return previous as T[];
  }
  return next as T[];
}

function reconcileArrayByKey<T>(
  previous: readonly T[] | undefined,
  next: readonly T[] | undefined,
  getKey: (entry: T) => string,
): T[] | undefined {
  if (!next) {
    return undefined;
  }
  if (!previous) {
    return next as T[];
  }
  const previousByKey = new Map(previous.map((entry) => [getKey(entry), entry]));
  let changed = previous.length !== next.length;
  const reconciled = next.map((entry) => {
    const prior = previousByKey.get(getKey(entry));
    if (prior && deepEqual(prior, entry)) {
      return prior;
    }
    changed = true;
    return entry;
  });
  if (!changed) {
    return previous as T[];
  }
  return reconciled;
}

function timelineItemKey(item: WebviewTimelineItem): string {
  switch (item.type) {
    case "approval":
      return `approval:${item.id}`;
    case "boundary":
      return `boundary:${item.id}`;
    case "checkpoint":
      return `checkpoint:${item.id}`;
    case "message":
      return `message:${item.id}`;
    case "plan":
      return `plan:${item.id}`;
    case "review":
      return `review:${item.id}`;
    case "thinking":
      return `thinking:${item.id}`;
    case "tool":
      return `tool:${item.id}`;
  }
}

function reconcileSessionTab(
  previous: WebviewSessionTab | undefined,
  next: WebviewSessionTab,
): WebviewSessionTab {
  return previous !== undefined && deepEqual(previous, next) ? previous : next;
}

export function reconcileSessionSnapshot(
  previous: WebviewSessionSnapshot | undefined,
  next: WebviewSessionSnapshot,
): WebviewSessionSnapshot {
  const nextPlanTodos = next.planTodos ?? [];
  const nextSessionTodos = next.sessionTodos ?? [];
  const nextPendingAttachments = next.pendingAttachments ?? [];
  const timeline = reconcileArrayByKey(previous?.timeline, next.timeline, timelineItemKey) ?? next.timeline;
  const checkpoints =
    reconcileArrayByKey(previous?.checkpoints, next.checkpoints, (entry: WebviewCheckpoint) => entry.id) ??
    next.checkpoints;
  const planTodos =
    reconcileArrayByKey(previous?.planTodos, nextPlanTodos, (entry: WebviewTodo) => entry.id) ??
    nextPlanTodos;
  const sessionTodos =
    reconcileArrayByKey(previous?.sessionTodos, nextSessionTodos, (entry: WebviewTodo) => entry.id) ??
    nextSessionTodos;
  const pendingAttachments =
    reconcileArrayByKey(
      previous?.pendingAttachments,
      nextPendingAttachments,
      (entry: WebviewPendingAttachment) => entry.id,
    ) ?? nextPendingAttachments;
  const planFile = reconcileValue(previous?.planFile, next.planFile);
  if (
    previous &&
    previous.busy === next.busy &&
    previous.contextRatio === next.contextRatio &&
    previous.hasMoreHistory === next.hasMoreHistory &&
    previous.historyLoading === next.historyLoading &&
    previous.model === next.model &&
    previous.thinkingLevel === next.thinkingLevel &&
    previous.ownedByThisFrontend === next.ownedByThisFrontend &&
    previous.planId === next.planId &&
    previous.planState === next.planState &&
    previous.sessionId === next.sessionId &&
    previous.timeline === timeline &&
    previous.checkpoints === checkpoints &&
    previous.planTodos === planTodos &&
    previous.sessionTodos === sessionTodos &&
    previous.pendingAttachments === pendingAttachments &&
    previous.planFile === planFile
  ) {
    return previous;
  }
  return {
    ...next,
    checkpoints,
    pendingAttachments,
    planFile,
    planTodos,
    sessionTodos,
    timeline,
  };
}

export function reconcileStateSnapshot(
  previous: WebviewStateSnapshot | null | undefined,
  next: WebviewStateSnapshot,
): WebviewStateSnapshot {
  if (!previous) {
    return next;
  }
  const sessions =
    reconcileArrayByKey(previous.sessions, next.sessions, (entry: WebviewSessionTab) => entry.sessionId) ??
    next.sessions;
  let sessionViewsChanged =
    Object.keys(previous.sessionViews).length !== Object.keys(next.sessionViews).length;
  const sessionViews: Record<string, WebviewSessionSnapshot> = {};
  for (const [sessionId, snapshot] of Object.entries(next.sessionViews)) {
    const reconciled = reconcileSessionSnapshot(previous.sessionViews[sessionId], snapshot);
    sessionViews[sessionId] = reconciled;
    if (previous.sessionViews[sessionId] !== reconciled) {
      sessionViewsChanged = true;
    }
  }
  const availableModels = reconcilePrimitiveArray(previous.availableModels, next.availableModels);
  const availableModelCapabilities = reconcileValue(
    previous.availableModelCapabilities,
    next.availableModelCapabilities,
  );
  const availableModelReasoningLevels = reconcileValue(
    previous.availableModelReasoningLevels,
    next.availableModelReasoningLevels,
  );
  if (
    previous.activeSessionId === next.activeSessionId &&
    previous.availableModels === availableModels &&
    previous.availableModelCapabilities === availableModelCapabilities &&
    previous.availableModelReasoningLevels === availableModelReasoningLevels &&
    previous.buildModel === next.buildModel &&
    previous.modelAdminSupported === next.modelAdminSupported &&
    previous.ready === next.ready &&
    previous.sessions === sessions &&
    !sessionViewsChanged
  ) {
    return previous;
  }
  return {
    ...next,
    availableModelCapabilities,
    availableModelReasoningLevels,
    availableModels,
    sessions,
    sessionViews,
  };
}

export function mergeSessionViewSnapshot(
  previous: WebviewStateSnapshot | null | undefined,
  input: {
    sessionId: string;
    tab?: WebviewSessionTab | null;
    view: WebviewSessionSnapshot;
  },
): WebviewStateSnapshot {
  if (!previous) {
    return {
      activeSessionId: null,
      availableModelCapabilities: {},
      availableModelReasoningLevels: {},
      availableModels: [],
      modelAdminSupported: false,
      ready: false,
      sessionViews: {
        [input.sessionId]: input.view,
      },
      sessions: input.tab ? [input.tab] : [],
    };
  }

  const nextView = reconcileSessionSnapshot(
    previous.sessionViews[input.sessionId],
    input.view,
  );
  const previousView = previous.sessionViews[input.sessionId];
  const sessionViews =
    previousView === nextView
      ? previous.sessionViews
      : {
          ...previous.sessionViews,
          [input.sessionId]: nextView,
        };

  let sessions = previous.sessions;
  if (input.tab) {
    const currentIndex = previous.sessions.findIndex(
      (entry) => entry.sessionId === input.sessionId,
    );
    const currentTab =
      currentIndex >= 0 ? previous.sessions[currentIndex] : undefined;
    const nextTab = reconcileSessionTab(currentTab, input.tab);
    if (currentIndex >= 0) {
      if (nextTab !== currentTab) {
        sessions = [...previous.sessions];
        sessions[currentIndex] = nextTab;
      }
    } else {
      sessions = [...previous.sessions, nextTab];
    }
  }

  if (sessionViews === previous.sessionViews && sessions === previous.sessions) {
    return previous;
  }

  return {
    ...previous,
    sessions,
    sessionViews,
  };
}
