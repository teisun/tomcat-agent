/** A single text-node portion of a rendered Find match. */
export interface PlanFindSegment {
  /** Text node containing this part of the match. */
  node: Text;
  /** Zero-based inclusive offset within `node.data`. */
  start: number;
  /** Zero-based exclusive offset within `node.data`. */
  end: number;
}

/**
 * A match may span adjacent inline nodes (for example `up<strong>date</strong>`)
 * but never crosses a rendered block boundary.
 */
export interface PlanFindMatch {
  segments: readonly PlanFindSegment[];
}

/** Same upper bound VS Code's Find model uses to keep huge documents responsive. */
export const PLAN_FIND_MATCHES_LIMIT = 19_999;

/**
 * Tags and element states that do not represent searchable plan copy. In
 * particular, Mermaid renders an SVG beside its source markdown, so searching
 * it would duplicate visible text and produce inaccessible results.
 */
const NON_SEARCHABLE_ANCESTOR_SELECTOR = [
  "script",
  "style",
  "template",
  "svg",
  "[hidden]",
  '[aria-hidden="true"]',
  ".tc-visually-hidden",
].join(", ");

/**
 * A Find result may join inline formatting, but it must not combine the tail of
 * one paragraph/list item with the beginning of the next. Markdown blocks are
 * source-mapped; ordinary todo/list/table blocks cover rendered non-Markdown
 * copy as well.
 */
const SEARCH_BLOCK_SELECTOR = [
  "[data-source-line]",
  "article",
  "aside",
  "blockquote",
  "dd",
  "div",
  "dt",
  "figcaption",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "li",
  "p",
  "pre",
  "section",
  "table",
  "td",
  "th",
  "ul",
].join(", ");

type TextSpan = {
  end: number;
  node: Text;
  start: number;
};

function isSearchableTextNode(root: HTMLElement, node: Text): boolean {
  let element = node.parentElement;
  if (!element || !root.contains(element)) {
    return false;
  }

  const view = root.ownerDocument.defaultView;
  while (element && root.contains(element)) {
    if (element.matches(NON_SEARCHABLE_ANCESTOR_SELECTOR)) {
      return false;
    }
    const style = view?.getComputedStyle(element);
    if (
      style?.display === "none" ||
      style?.visibility === "hidden" ||
      style?.visibility === "collapse"
    ) {
      return false;
    }
    if (element === root) {
      break;
    }
    element = element.parentElement;
  }
  return true;
}

function collectSearchableTextGroups(root: HTMLElement): TextSpan[][] {
  const blocks: TextSpan[][] = [];

  const appendText = (spans: TextSpan[], node: Text) => {
    const start = spans.length === 0 ? 0 : spans[spans.length - 1].end;
    spans.push({ end: start + node.data.length, node, start });
  };

  const visitChildren = (
    element: Element,
    activeGroup: TextSpan[] | null,
  ): TextSpan[] | null => {
    let group = activeGroup;
    for (const child of element.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) {
        const node = child as Text;
        if (node.data.length > 0 && isSearchableTextNode(root, node)) {
          if (!group) {
            group = [];
            blocks.push(group);
          }
          appendText(group, node);
        }
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) {
        continue;
      }

      const childElement = child as HTMLElement;
      if (childElement.matches(SEARCH_BLOCK_SELECTOR)) {
        // Even an empty nested block is a visible document boundary. It must
        // split direct text before/after it instead of being invisible to a
        // text-only TreeWalker.
        visitChildren(childElement, null);
        group = null;
      } else {
        group = visitChildren(childElement, group);
      }
    }
    return group;
  };

  visitChildren(root, null);
  return blocks;
}

function segmentsForMatch(
  spans: readonly TextSpan[],
  start: number,
  end: number,
): PlanFindSegment[] {
  const segments: PlanFindSegment[] = [];
  for (const span of spans) {
    if (span.end <= start) {
      continue;
    }
    if (span.start >= end) {
      break;
    }
    const segmentStart = Math.max(0, start - span.start);
    const segmentEnd = Math.min(span.node.data.length, end - span.start);
    if (segmentEnd > segmentStart) {
      segments.push({ end: segmentEnd, node: span.node, start: segmentStart });
    }
  }
  return segments;
}

/**
 * Finds case-insensitive, non-overlapping substring matches in rendered plan
 * copy. Adjacent inline text nodes in the same rendered block are reconstructed
 * into one logical string, then each logical offset is mapped back to the exact
 * text-node ranges required by CSS Custom Highlight. Blocks are searched
 * independently, which is an explicit non-matchable separator between them.
 */
export function collectPlanFindMatches(
  root: HTMLElement,
  query: string,
  matchesLimit = PLAN_FIND_MATCHES_LIMIT,
): PlanFindMatch[] {
  if (query.length === 0) {
    return [];
  }

  const limit = Math.max(0, Math.floor(matchesLimit));
  if (limit === 0) {
    return [];
  }

  const blocks = collectSearchableTextGroups(root);

  const needle = query.toLowerCase();
  const matches: PlanFindMatch[] = [];
  for (const spans of blocks) {
    const haystack = spans.map((span) => span.node.data).join("").toLowerCase();
    let start = haystack.indexOf(needle);
    while (start !== -1) {
      const end = start + query.length;
      const segments = segmentsForMatch(spans, start, end);
      if (segments.length > 0) {
        matches.push({ segments });
      }
      if (matches.length >= limit) {
        return matches;
      }
      start = haystack.indexOf(needle, end);
    }
  }

  return matches;
}
