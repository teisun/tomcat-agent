import DOMPurify from "dompurify";
import { marked } from "marked";
import { useEffect, useMemo, useRef, type MouseEvent } from "react";

/**
 * Render mermaid fenced code blocks (```mermaid```) into inline SVG diagrams,
 * matching Cursor's plan preview. Mermaid is loaded lazily (its own chunk) so
 * plans without diagrams pay nothing, and rendering runs against the already
 * sanitized DOM. Failures fall back to the original code block untouched.
 */
async function renderMermaidBlocks(container: HTMLElement, isCancelled: () => boolean): Promise<void> {
  const blocks = Array.from(container.querySelectorAll<HTMLElement>("code.language-mermaid"));
  if (blocks.length === 0) {
    return;
  }
  const mermaid = (await import("mermaid")).default;
  if (isCancelled()) {
    return;
  }
  const dark =
    document.body.classList.contains("vscode-dark") ||
    document.body.classList.contains("vscode-high-contrast");
  mermaid.initialize({
    fontFamily: "var(--vscode-font-family, sans-serif)",
    securityLevel: "strict",
    startOnLoad: false,
    theme: dark ? "dark" : "default",
  });

  for (const [index, code] of blocks.entries()) {
    const host = code.closest("pre") ?? code;
    const graph = code.textContent ?? "";
    const id = `tc-mermaid-${Date.now().toString(36)}-${index}`;
    try {
      const { svg } = await mermaid.render(id, graph);
      if (isCancelled()) {
        return;
      }
      const figure = document.createElement("figure");
      figure.className = "tc-plan-preview__mermaid";
      figure.setAttribute("data-testid", "plan-mermaid");
      figure.innerHTML = svg;
      host.replaceWith(figure);
    } catch {
      if (!isCancelled()) {
        host.setAttribute("data-mermaid-error", "1");
      }
    }
  }
}

/**
 * Render the plan body markdown as sanitized HTML. Links never navigate the
 * webview directly — clicks are intercepted and forwarded to the host via
 * `onOpenLink`, matching the strict CSP (no inline scripts, no navigation).
 */
export function MarkdownBody({
  markdown,
  onOpenLink,
}: {
  markdown: string;
  onOpenLink(href: string): void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const html = useMemo(() => {
    const rendered = marked.parse(markdown, { async: false, gfm: true }) as string;
    return DOMPurify.sanitize(rendered, { USE_PROFILES: { html: true } });
  }, [markdown]);

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
