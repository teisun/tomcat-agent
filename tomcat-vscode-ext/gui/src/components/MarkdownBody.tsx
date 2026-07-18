import { useEffect, useMemo, useRef, type MouseEvent } from "react";
import {
  renderMarkdownHtml,
  renderMermaidBlocks,
  sanitizeMarkdownHtml,
} from "./markdown/markdownRuntime";
import { linkifyInlineFilePaths } from "./markdown/markdownDecorators";

/**
 * Render the plan body markdown as sanitized HTML. Links never navigate the
 * webview directly — clicks are intercepted and forwarded to the host via
 * `onOpenLink`, matching the strict CSP (no inline scripts, no navigation).
 */
export function MarkdownBody({
  markdown,
  onOpenFile,
  onOpenLink,
  sourceLineMap,
}: {
  markdown: string;
  onOpenFile?(path: string, line?: number): void;
  onOpenLink(href: string): void;
  /** 1-based source file line for each line of `markdown` (see planDocument). */
  sourceLineMap?: number[];
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const html = useMemo(() => {
    const rendered = sanitizeMarkdownHtml(renderMarkdownHtml(markdown, sourceLineMap));
    if (typeof document === "undefined") {
      return rendered;
    }
    const container = document.createElement("div");
    container.innerHTML = rendered;
    linkifyInlineFilePaths(container);
    return container.innerHTML;
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
    const fileTarget = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-tc-file-path]");
    if (fileTarget) {
      event.preventDefault();
      event.stopPropagation();
      const line = fileTarget.dataset.tcLine ? Number(fileTarget.dataset.tcLine) : undefined;
      onOpenFile?.(fileTarget.dataset.tcFilePath ?? "", Number.isFinite(line) ? line : undefined);
      return;
    }
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
