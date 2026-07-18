import { useEffect, useMemo, useRef, type MouseEvent } from "react";
import {
  renderMarkdownHtml,
  renderMermaidBlocks,
  sanitizeMarkdownHtml,
} from "./markdown/markdownRuntime";

/**
 * Render the plan body markdown as sanitized HTML. Links never navigate the
 * webview directly — clicks are intercepted and forwarded to the host via
 * `onOpenLink`, matching the strict CSP (no inline scripts, no navigation).
 */
export function MarkdownBody({
  markdown,
  onOpenLink,
  sourceLineMap,
}: {
  markdown: string;
  onOpenLink(href: string): void;
  /** 1-based source file line for each line of `markdown` (see planDocument). */
  sourceLineMap?: number[];
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const html = useMemo(() => {
    return sanitizeMarkdownHtml(renderMarkdownHtml(markdown, sourceLineMap));
  }, [markdown, sourceLineMap]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    let cancelled = false;
    void renderMermaidBlocks(container, () => cancelled);
    return () => {
      cancelled = true;
    };
  }, [html]);

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    const anchor = (event.target as HTMLElement | null)?.closest("a");
    if (!anchor) {
      return;
    }
    event.preventDefault();
    const href = anchor.getAttribute("href");
    if (href) {
      onOpenLink(href);
    }
  };

  return (
    <div
      className="tc-plan-preview__body"
      data-testid="plan-markdown-body"
      dangerouslySetInnerHTML={{ __html: html }}
      onClick={handleClick}
      ref={containerRef}
    />
  );
}
