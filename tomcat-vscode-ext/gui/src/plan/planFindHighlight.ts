import type { PlanFindMatch, PlanFindSegment } from "./planFindEngine";

const FIND_CHROME_SELECTOR = ".tc-plan-find, .tc-plan-action-strip";
const FALLBACK_MATCH_CLASS = "tc-plan-find-fallback-highlight";
const FALLBACK_ACTIVE_CLASS = "tc-plan-find-fallback-highlight--active";

type DomHighlights = {
  activeElements: HTMLElement[];
  elementsByMatch: Map<PlanFindMatch, HTMLElement[]>;
};

type SegmentToWrap = {
  match: PlanFindMatch;
  segment: PlanFindSegment;
};

let domHighlights: DomHighlights | null = null;
let decorationMutationTimer: ReturnType<typeof setTimeout> | null = null;
let decorationMutationInProgress = false;

function rangeFor(segment: PlanFindSegment): Range {
  const range = segment.node.ownerDocument.createRange();
  range.setStart(segment.node, segment.start);
  range.setEnd(segment.node, segment.end);
  return range;
}

/** The first range is the stable anchor before DOM fallback decoration is applied. */
export function firstPlanFindRange(match: PlanFindMatch): Range | null {
  const segment = match.segments[0];
  if (!segment) {
    return null;
  }
  try {
    return rangeFor(segment);
  } catch {
    // DOM fallback decoration may have split this source node, or a renderer
    // may have replaced it between collection and navigation. A live span is
    // preferred above; without one, skip one reveal rather than throw from a
    // keyboard handler or applying an offset to unrelated content.
    return null;
  }
}
function flagDecorationMutation(): void {
  decorationMutationInProgress = true;
  if (decorationMutationTimer !== null) {
    clearTimeout(decorationMutationTimer);
  }
  // MutationObserver delivers after this synchronous decoration work. Keeping the
  // flag through the following task prevents Find from treating its own spans as
  // a Markdown/Mermaid content replacement and repeatedly re-searching itself.
  decorationMutationTimer = setTimeout(() => {
    decorationMutationInProgress = false;
    decorationMutationTimer = null;
  }, 0);
}

/** Used by the rendered-content observer to ignore only Find's own span changes. */
export function isPlanFindDecorationMutationInProgress(): boolean {
  return decorationMutationInProgress;
}

function clearFallbackHighlights(): void {
  const current = domHighlights;
  domHighlights = null;
  if (!current) {
    return;
  }

  flagDecorationMutation();
  for (const elements of current.elementsByMatch.values()) {
    for (const highlight of elements) {
      if (!highlight.isConnected) {
        continue;
      }
      // Restore the original text nodes. No Find span remains in the document
      // after a new query, close, or rendered-content replacement.
      highlight.replaceWith(...Array.from(highlight.childNodes));
    }
  }
}

function wrapFallbackHighlights(
  matches: readonly PlanFindMatch[],
  activeIndex: number,
): void {
  const elementsByMatch = new Map<PlanFindMatch, HTMLElement[]>();
  const segmentsByNode = new Map<Text, SegmentToWrap[]>();

  for (const match of matches) {
    elementsByMatch.set(match, []);
    for (const segment of match.segments) {
      const segments = segmentsByNode.get(segment.node) ?? [];
      segments.push({ match, segment });
      segmentsByNode.set(segment.node, segments);
    }
  }

  flagDecorationMutation();
  // `surroundContents` splits text nodes. Process each node right-to-left so
  // an earlier range keeps the offsets discovered by the Find engine.
  for (const segments of segmentsByNode.values()) {
    segments.sort((left, right) => right.segment.start - left.segment.start);
    for (const { match, segment } of segments) {
      const highlight = segment.node.ownerDocument.createElement("span");
      highlight.className = FALLBACK_MATCH_CLASS;
      try {
        rangeFor(segment).surroundContents(highlight);
      } catch {
        // A DOM transform may have replaced the text between collection and
        // paint. The next content refresh will collect current matches again.
        continue;
      }
      elementsByMatch.get(match)?.push(highlight);
    }
  }

  domHighlights = { activeElements: [], elementsByMatch };
  setPlanFindActiveHighlight(matches, activeIndex);
}

/** Remove Plan Preview Find decorations and restore the original text nodes. */
export function clearPlanFindHighlights(): void {
  clearFallbackHighlights();
}

/**
 * Changes only the current match's class. All ordinary candidates remain in the
 * document, matching VS Code's persistent match decorations plus one current
 * match decoration.
 */
export function setPlanFindActiveHighlight(
  matches: readonly PlanFindMatch[],
  activeIndex: number,
): void {
  const highlights = domHighlights;
  const activeMatch = matches[activeIndex];
  if (!highlights || !activeMatch) {
    return;
  }

  for (const element of highlights.activeElements) {
    element.classList.remove(FALLBACK_ACTIVE_CLASS);
  }
  const activeElements = highlights.elementsByMatch.get(activeMatch) ?? [];
  for (const element of activeElements) {
    element.classList.add(FALLBACK_ACTIVE_CLASS);
  }
  highlights.activeElements = activeElements;
}

/**
 * Highlight the current result set with ordinary inline spans and mark one as
 * active. Unlike fixed geometry overlays, inline spans are laid out by the
 * browser with their text and cannot drift, float, or repaint on scrolling.
 */
export function paintPlanFindHighlights(
  matches: readonly PlanFindMatch[],
  activeIndex: number,
): void {
  clearPlanFindHighlights();
  if (matches.length === 0) {
    return;
  }
  wrapFallbackHighlights(matches, activeIndex);
}

function firstRenderedMatchElement(match: PlanFindMatch): HTMLElement | null {
  const elements = domHighlights?.elementsByMatch.get(match);
  return elements?.find((element) => element.isConnected) ?? null;
}

/**
 * Returns the part of the content viewport obscured by fixed Plan chrome. The
 * action strip normally consumes layout space, but including it here makes the
 * calculation correct if future styles turn it into an overlay.
 */
function chromeInset(container: HTMLElement, containerRect: DOMRect): number {
  const viewportBottom = containerRect.bottom;
  let inset = 0;
  for (const chrome of container.ownerDocument.querySelectorAll<HTMLElement>(
    FIND_CHROME_SELECTOR,
  )) {
    const rect = chrome.getBoundingClientRect();
    if (rect.bottom <= containerRect.top || rect.top >= viewportBottom) {
      continue;
    }
    inset = Math.max(inset, rect.bottom - containerRect.top);
  }
  return Math.max(0, inset);
}

/**
 * Place an explicitly selected Find match at the readable viewport centre.
 * After fallback spans exist, their real layout box is authoritative; this
 * avoids navigation using a stale pre-decoration Range.
 */
export function centerPlanFindMatch(
  match: PlanFindMatch,
  container: HTMLElement,
): void {
  const renderedElement = firstRenderedMatchElement(match);
  if (renderedElement && typeof renderedElement.scrollIntoView === "function") {
    // This is the browser equivalent of VS Code's
    // `revealRangeInCenterIfOutsideViewport`: let the layout engine reveal the
    // real inline span through its true scroll ancestors instead of recreating
    // our own scroll-coordinate model.
    renderedElement.scrollIntoView({ block: "center", inline: "nearest" });
    return;
  }

  const range = renderedElement ? null : firstPlanFindRange(match);
  if (
    !renderedElement &&
    (!range || typeof range.getBoundingClientRect !== "function")
  ) {
    return;
  }

  const matchRect = renderedElement
    ? renderedElement.getBoundingClientRect()
    : range!.getBoundingClientRect();  const containerRect = container.getBoundingClientRect();
  const viewportHeight = container.clientHeight || containerRect.height;
  if (viewportHeight <= 0) {
    return;
  }

  const visibleTop = containerRect.top + chromeInset(container, containerRect);
  const visibleBottom = containerRect.top + viewportHeight;
  const targetCenter = (visibleTop + visibleBottom) / 2;
  const matchCenter = (matchRect.top + matchRect.bottom) / 2;
  const requestedScrollTop = container.scrollTop + matchCenter - targetCenter;
  const maxScrollTop = container.scrollHeight - viewportHeight;
  container.scrollTop =
    maxScrollTop > 0
      ? Math.min(Math.max(0, requestedScrollTop), maxScrollTop)
      : Math.max(0, requestedScrollTop);
}
