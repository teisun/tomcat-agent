import DOMPurify from "dompurify";
import { Marked, Renderer, type Token, type Tokens } from "marked";
import { useEffect, useMemo, useRef, type MouseEvent } from "react";

/**
 * Render markdown while tagging each top-level block element with a
 * `data-source-line` attribute holding its 1-based line in the original plan
 * file. This mirrors VS Code's built-in markdown preview (`data-line` +
 * `code-line`) and lets a rendered text selection map back to exact source
 * lines regardless of inline formatting.
 *
 * marked (unlike markdown-it) exposes no `token.map`, so we derive each block's
 * body-relative start line by accumulating newlines in `token.raw`, then look up
 * the absolute file line via `sourceLineMap`. marked v16 ignores a subclassed
 * Renderer passed to `use()` (it only copies own-enumerable methods), so we pass
 * plain methods that delegate to `Renderer.prototype[name]` for default output.
 */
const BLOCK_RENDERERS = [
  "heading",
  "paragraph",
  "list",
  "code",
  "blockquote",
  "hr",
  "table",
] as const;

type TokenWithSourceLine = Token & { _sourceLine?: number };

function injectSourceLine(html: string, line: number | undefined): string {
  if (!line || line <= 0) {
    return html;
  }
  return html.replace(/^(\s*)<([a-zA-Z][\w-]*)/u, `$1<$2 data-source-line="${line}"`);
}

function createSourceLineMarked(): Marked {
  const proto = Renderer.prototype as unknown as Record<
    string,
    (token: TokenWithSourceLine) => string
  >;
  const renderer: Record<string, (token: TokenWithSourceLine) => string> = {};
  for (const name of BLOCK_RENDERERS) {
    renderer[name] = function rendererWithSourceLine(
      this: Renderer,
      token: TokenWithSourceLine,
    ): string {
      const base = proto[name].call(this, token);
      return injectSourceLine(base, token._sourceLine);
    };
  }
  const instance = new Marked({ gfm: true });
  instance.use({ renderer });
  return instance;
}

const markedInstance = createSourceLineMarked();

function renderPlanMarkdown(markdown: string, sourceLineMap?: number[]): string {
  const tokens = markedInstance.lexer(markdown) as Tokens.Generic[] as TokenWithSourceLine[];
  if (sourceLineMap && sourceLineMap.length > 0) {
    let bodyLine = 1;
    for (const token of tokens) {
      const absolute = sourceLineMap[bodyLine - 1];
      if (typeof absolute === "number") {
        token._sourceLine = absolute;
      }
      bodyLine += (token.raw.match(/\n/gu) ?? []).length;
    }
  }
  return markedInstance.parser(tokens as unknown as Token[]);
}

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
  sourceLineMap,
}: {
  markdown: string;
  onOpenLink(href: string): void;
  /** 1-based source file line for each line of `markdown` (see planDocument). */
  sourceLineMap?: number[];
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const html = useMemo(() => {
    const rendered = renderPlanMarkdown(markdown, sourceLineMap);
    return DOMPurify.sanitize(rendered, {
      ADD_ATTR: ["data-source-line"],
      USE_PROFILES: { html: true },
    });
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
