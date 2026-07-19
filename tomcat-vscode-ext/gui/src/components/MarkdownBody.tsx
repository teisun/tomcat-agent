import { useEffect, useMemo, useRef, type MouseEvent } from "react";
import { buildDecoratedHtml, flashCopyButton } from "./markdown/markdownDecorators";
import { renderMermaidBlocks } from "./markdown/markdownRuntime";

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
  const html = useMemo(() => buildDecoratedHtml(markdown, sourceLineMap), [markdown, sourceLineMap]);

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
    const target = event.target as HTMLElement | null;
    const copyButton = target?.closest<HTMLElement>("[data-tc-copy-code]");
    if (copyButton) {
      event.preventDefault();
      event.stopPropagation();
      const card = copyButton.closest(".tc-code-card");
      const codeText = card?.querySelector("pre code")?.textContent ?? "";
      if (typeof navigator?.clipboard?.writeText === "function") {
        void navigator.clipboard.writeText(codeText).then(
          () => flashCopyButton(copyButton),
          () => undefined,
        );
      }
      return;
    }
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
