import {
  collectPlanFindMatches,
  PLAN_FIND_MATCHES_LIMIT,
  type PlanFindMatch,
} from "./planFindEngine";
import {
  centerPlanFindMatch,
  clearPlanFindHighlights,
  firstPlanFindRange,
  paintPlanFindHighlights,
  setPlanFindActiveHighlight,
} from "./planFindHighlight";
export { PLAN_FIND_MATCHES_LIMIT } from "./planFindEngine";

/** Same input settling delay used by VS Code's Find model. */
export const PLAN_FIND_RESEARCH_DELAY = 240;

export interface PlanFindSnapshot {
  activeIndex: number;
  matches: readonly PlanFindMatch[];
  open: boolean;
  query: string;
}

export interface PlanFindControllerOptions {
  /** Kept injectable so controller tests can run without waiting for the UI delay. */
  researchDelayMs?: number;
  /** Kept injectable for a small, focused result-cap test. */
  matchesLimit?: number;
}

type ChangeListener = () => void;

const CLOSED_SNAPSHOT: PlanFindSnapshot = {
  activeIndex: 0,
  matches: [],
  open: false,
  query: "",
};

/**
 * Framework-independent state and orchestration for Plan Preview Find.
 *
 * React only subscribes to this object. The controller owns the find lifecycle:
 * collecting matches, applying browser highlights, moving the active result and
 * bringing it into view. That is deliberately the same boundary as VS Code's
 * FindReplaceState + FindModel pairing.
 */
export class PlanFindController {
  private contentVersion: unknown;
  private disposed = false;
  private readonly listeners = new Set<ChangeListener>();
  private readonly matchesLimit: number;
  private researchTimer: ReturnType<typeof setTimeout> | null = null;
  private reanchorOnNextRefresh = false;
  /** True only after an explicit query or next/previous selection action. */
  private revealSelectedMatchOnNextRefresh = false;
  private readonly researchDelayMs: number;
  private root: HTMLElement | null = null;
  private snapshot: PlanFindSnapshot = CLOSED_SNAPSHOT;
  constructor(options: PlanFindControllerOptions = {}) {
    this.matchesLimit = Math.max(1, options.matchesLimit ?? PLAN_FIND_MATCHES_LIMIT);
    this.researchDelayMs = Math.max(0, options.researchDelayMs ?? PLAN_FIND_RESEARCH_DELAY);
  }

  public getSnapshot = (): PlanFindSnapshot => this.snapshot;

  public subscribe = (listener: ChangeListener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Supplies the live scroll/search root after React has committed it. */
  public setSearchRoot = (root: HTMLElement | null): void => {
    if (this.disposed || this.root === root) {
      return;
    }
    this.root = root;
    this.reanchorOnNextRefresh = true;
    this.scheduleRefresh();
  };

  /** Re-search only when the rendered plan body/todos version actually changes. */
  public setContentVersion = (contentVersion: unknown): void => {
    if (this.disposed || Object.is(this.contentVersion, contentVersion)) {
      return;
    }
    this.contentVersion = contentVersion;
    this.scheduleRefresh();
  };

  /** Re-search after asynchronous Markdown DOM transforms (paths/Mermaid) settle. */
  public notifyRenderedContentChanged = (): void => {
    if (this.disposed) {
      return;
    }
    this.scheduleRefresh();
  };

  public open = (): void => {
    if (this.disposed || this.snapshot.open) {
      return;
    }
    this.publish({ ...this.snapshot, open: true });
    this.reanchorOnNextRefresh = true;
    this.scheduleRefresh();
  };

  public close = (): void => {
    if (this.disposed) {
      return;
    }
    this.clearResearchTimer();
    clearPlanFindHighlights();
    this.publish(CLOSED_SNAPSHOT);
  };

  public setQuery = (query: string): void => {
    if (this.disposed || this.snapshot.query === query) {
      return;
    }
    // Drop old results immediately: their count/highlight belongs to a different
    // term and must not be presented while the debounced search settles.
    this.publish({
      activeIndex: 0,
      matches: [],
      open: this.snapshot.open,
      query,
    });
    clearPlanFindHighlights();
    this.reanchorOnNextRefresh = true;
    this.revealSelectedMatchOnNextRefresh = true;
    this.scheduleRefresh();  };

  public moveNext = (): void => this.move(1);

  public movePrev = (): void => this.move(-1);

  /** Runs a search immediately; mainly used by lifecycle code and unit tests. */
  public refresh = (): void => {
    this.clearResearchTimer();
    if (this.disposed) {
      return;
    }

    const { open, query } = this.snapshot;
    if (!open || query.length === 0 || !this.root) {
      clearPlanFindHighlights();
      if (this.snapshot.matches.length !== 0 || this.snapshot.activeIndex !== 0) {
        this.publish({ ...this.snapshot, activeIndex: 0, matches: [] });
      }
      return;
    }

    const matches = collectPlanFindMatches(this.root, query, this.matchesLimit);
    const activeIndex =
      matches.length === 0
        ? 0
        : this.reanchorOnNextRefresh
          ? this.closestMatchToViewport(matches)
          : Math.min(this.snapshot.activeIndex, matches.length - 1);
    const revealSelectedMatch = this.revealSelectedMatchOnNextRefresh;
    this.reanchorOnNextRefresh = false;
    this.revealSelectedMatchOnNextRefresh = false;
    this.publish({ ...this.snapshot, activeIndex, matches });
    this.renderActiveMatch(true, revealSelectedMatch);
  };

  public dispose = (): void => {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.clearResearchTimer();
    clearPlanFindHighlights();
    this.listeners.clear();
    this.root = null;
  };

  private move(direction: -1 | 1): void {
    if (this.disposed || this.snapshot.matches.length === 0) {
      return;
    }
    const total = this.snapshot.matches.length;
    const activeIndex = (this.snapshot.activeIndex + direction + total) % total;
    this.publish({ ...this.snapshot, activeIndex });
    this.renderActiveMatch(false, true);
  }

  private scheduleRefresh(): void {
    this.clearResearchTimer();
    if (
      this.disposed ||
      !this.snapshot.open ||
      this.snapshot.query.length === 0 ||
      !this.root
    ) {
      return;
    }
    this.researchTimer = setTimeout(() => {
      this.researchTimer = null;
      this.refresh();
    }, this.researchDelayMs);
  }

  private clearResearchTimer(): void {
    if (this.researchTimer !== null) {
      clearTimeout(this.researchTimer);
      this.researchTimer = null;
    }
  }

  private publish(snapshot: PlanFindSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) {
      listener();
    }
  }

  private renderActiveMatch(
    matchesChanged: boolean,
    revealSelectedMatch = false,
  ): void {
    const { activeIndex, matches } = this.snapshot;
    const activeMatch = matches[activeIndex];
    if (revealSelectedMatch && activeMatch && this.root) {
      // VS Code's FindModel keeps decorations separate and reveals only the
      // selected match. Plan Preview intentionally centres every explicit
      // selection (query settle, Next, Previous), never ordinary user scrolling.
      centerPlanFindMatch(activeMatch, this.root);
    }
    if (matchesChanged) {
      paintPlanFindHighlights(matches, activeIndex);
    } else {
      setPlanFindActiveHighlight(matches, activeIndex);
    }
  }
  /**
   * Match order follows DOM order, and rendered ranges therefore progress down
   * the document. A lower-bound binary search picks the first match at or below
   * the visible top edge; jsdom/no-layout safely falls back to the first result.
   */
  private closestMatchToViewport(matches: readonly PlanFindMatch[]): number {
    if (matches.length <= 1 || !this.root) {
      return 0;
    }
    const top = this.root.getBoundingClientRect().top;
    let low = 0;
    let high = matches.length;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      const matchTop = this.matchTop(matches[middle]);
      if (matchTop === null) {
        return 0;
      }
      if (matchTop < top) {
        low = middle + 1;
      } else {
        high = middle;
      }
    }
    return Math.min(low, matches.length - 1);
  }

  private matchTop(match: PlanFindMatch): number | null {
    const range = firstPlanFindRange(match);
    if (!range || typeof range.getBoundingClientRect !== "function") {
      return null;
    }
    return range.getBoundingClientRect().top;
  }
}
