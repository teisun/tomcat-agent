import {
  forwardRef,
  Fragment,
  useEffect,
  useImperativeHandle,
  useState,
} from "react";

import type { ContextSearchMatch } from "../types";

export interface ContextSearchDropdownHandle {
  onKeyDown(event: KeyboardEvent): boolean;
}

interface ContextSearchDropdownProps {
  loading: boolean;
  matches: ContextSearchMatch[];
  onSelect(match: ContextSearchMatch): void;
  open: boolean;
  query: string;
  truncated: boolean;
}

function iconClassName(match: ContextSearchMatch): string {
  return match.reference.path.endsWith("/")
    ? "codicon codicon-folder"
    : "codicon codicon-file";
}

function optionIdForMatch(match: ContextSearchMatch, index: number): string {
  return `context-search-option-${match.reference.path.replace(/[^a-zA-Z0-9_-]+/g, "-")}-${index}`;
}

function renderHighlightedLabel(label: string, query: string) {
  const trimmedQuery = query.trim();
  if (!trimmedQuery) {
    return label;
  }

  const lowerLabel = label.toLowerCase();
  const lowerQuery = trimmedQuery.toLowerCase();
  const matchedIndices = new Set<number>();
  let queryIndex = 0;
  for (let labelIndex = 0; labelIndex < lowerLabel.length; labelIndex += 1) {
    if (queryIndex >= lowerQuery.length) {
      break;
    }
    if (lowerLabel[labelIndex] === lowerQuery[queryIndex]) {
      matchedIndices.add(labelIndex);
      queryIndex += 1;
    }
  }
  if (queryIndex < lowerQuery.length) {
    return label;
  }

  const parts: Array<string | { highlighted: true; text: string }> = [];
  let segmentStart = 0;
  let highlighted = matchedIndices.has(0);
  for (let index = 0; index <= label.length; index += 1) {
    const nextHighlighted = matchedIndices.has(index);
    if (index < label.length && nextHighlighted === highlighted) {
      continue;
    }
    const text = label.slice(segmentStart, index);
    if (text) {
      parts.push(highlighted ? { highlighted: true, text } : text);
    }
    segmentStart = index;
    highlighted = nextHighlighted;
  }

  return parts.map((part, index) => {
    if (!part) {
      return null;
    }
    return typeof part === "string" ? (
      <Fragment key={`${label}-${index}`}>{part}</Fragment>
    ) : (
      <span className="tc-context-search-dropdown__highlight" key={`${label}-${index}`}>
        {part.text}
      </span>
    );
  });
}

export const ContextSearchDropdown = forwardRef<
  ContextSearchDropdownHandle,
  ContextSearchDropdownProps
>(function ContextSearchDropdown({
  loading,
  matches,
  onSelect,
  open,
  query,
  truncated,
}, ref) {
  const [selectedIndex, setSelectedIndex] = useState(0);
  const activeOptionId = matches[selectedIndex]
    ? optionIdForMatch(matches[selectedIndex], selectedIndex)
    : undefined;

  useEffect(() => {
    if (!open) {
      setSelectedIndex(0);
      return;
    }
    setSelectedIndex(0);
  }, [open, query]);

  useEffect(() => {
    setSelectedIndex((current) => {
      if (matches.length === 0) {
        return 0;
      }
      return Math.min(current, matches.length - 1);
    });
  }, [matches]);

  useImperativeHandle(ref, () => ({
    onKeyDown(event: KeyboardEvent): boolean {
      if (!open) {
        return false;
      }
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          if (matches.length > 0) {
            setSelectedIndex((current) => (current + 1) % matches.length);
          }
          return true;
        case "ArrowUp":
          event.preventDefault();
          if (matches.length > 0) {
            setSelectedIndex((current) => (current - 1 + matches.length) % matches.length);
          }
          return true;
        case "Enter":
        case "Tab": {
          event.preventDefault();
          const match = matches[selectedIndex];
          if (match) {
            onSelect(match);
          }
          return true;
        }
        case "Escape":
        case "Esc":
          event.preventDefault();
          return true;
        default:
          return false;
      }
    },
  }), [matches, onSelect, open, selectedIndex]);

  if (!open) {
    return null;
  }

  return (
    <div
      className="tc-session-dropdown tc-context-search-dropdown"
      aria-activedescendant={activeOptionId}
      aria-busy={loading}
      data-testid="context-search-dropdown"
      role="listbox"
    >
      {loading && matches.length === 0 ? (
        <div className="tc-session-dropdown__empty" data-testid="context-search-loading">
          搜索中
        </div>
      ) : matches.length === 0 ? (
        <div className="tc-session-dropdown__empty" data-testid="context-search-empty">
          未找到匹配文件
        </div>
      ) : (
        <div className="tc-context-search-dropdown__list">
          {matches.map((match, index) => {
            const isActive = index === selectedIndex;
            const optionId = optionIdForMatch(match, index);
            return (
              <button
                aria-selected={isActive}
                className={`tc-session-item tc-context-search-dropdown__item${
                  isActive ? " tc-session-item--active" : ""
                }`}
                data-testid="context-search-option"
                id={optionId}
                key={`${match.reference.path}:${index}`}
                onClick={() => onSelect(match)}
                onMouseDown={(event) => event.preventDefault()}
                role="option"
                title={match.reference.path}
                type="button"
              >
                <span
                  aria-hidden="true"
                  className={`tc-context-search-dropdown__icon ${iconClassName(match)}`}
                />
                <span className="tc-session-item__title">
                  {renderHighlightedLabel(match.reference.label, query)}
                </span>
                {match.description ? (
                  <span className="tc-context-search-dropdown__description">
                    {match.description}
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
      )}
      {loading && matches.length > 0 ? (
        <div
          className="tc-context-search-dropdown__footer tc-context-search-dropdown__footer--status"
          data-testid="context-search-loading-inline"
        >
          搜索中
        </div>
      ) : null}
      {truncated ? (
        <div className="tc-context-search-dropdown__footer" data-testid="context-search-truncated">
          {`仅显示前 ${matches.length} 条，输入更精确关键词`}
        </div>
      ) : null}
    </div>
  );
});
