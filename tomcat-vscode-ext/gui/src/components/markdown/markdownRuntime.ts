import DOMPurify from "dompurify";
import { Marked, Renderer, type Token, type Tokens } from "marked";

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

function escapeHtmlAttribute(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;");
}

function injectAttribute(html: string, name: string, value: string | number | undefined): string {
  if (value === undefined || value === null || value === "") {
    return html;
  }
  return html.replace(
    /^(\s*)<([a-zA-Z][\w-]*)/u,
    `$1<$2 ${name}="${escapeHtmlAttribute(String(value))}"`,
  );
}

function injectSourceLine(html: string, line: number | undefined): string {
  return typeof line === "number" && line > 0 ? injectAttribute(html, "data-source-line", line) : html;
}

function extractFenceInfoFromRaw(raw: string): string | undefined {
  const firstLine = raw.split("\n", 1)[0]?.trim() ?? "";
  if (!firstLine) {
    return undefined;
  }
  const match = firstLine.match(/^(?:`{3,}|~{3,})\s*(.*)$/u);
  const info = match?.[1]?.trim();
  return info ? info : undefined;
}

function createMarkedWithSourceLines(): Marked {
  const proto = Renderer.prototype as unknown as Record<
    string,
    (token: TokenWithSourceLine) => string
  >;
  const renderer: Record<string, (token: TokenWithSourceLine) => string> = {};

  for (const name of BLOCK_RENDERERS) {
    renderer[name] = function rendererWithMetadata(
      this: Renderer,
      token: TokenWithSourceLine,
    ): string {
      let html = proto[name].call(this, token);
      html = injectSourceLine(html, token._sourceLine);
      if (name === "code") {
        html = injectAttribute(html, "data-fence-info", extractFenceInfoFromRaw(token.raw));
      }
      return html;
    };
  }

  const instance = new Marked({ gfm: true });
  instance.use({ renderer });
  return instance;
}

const markedInstance = createMarkedWithSourceLines();

export function renderMarkdownHtml(markdown: string, sourceLineMap?: number[]): string {
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

export function sanitizeMarkdownHtml(renderedHtml: string): string {
  return DOMPurify.sanitize(renderedHtml, {
    ADD_ATTR: ["data-fence-info", "data-source-line"],
    USE_PROFILES: { html: true },
  });
}

/**
 * Render mermaid fenced code blocks (```mermaid```) into inline SVG diagrams.
 * The import stays lazy so regular markdown pays nothing for mermaid.
 */
export async function renderMermaidBlocks(
  container: HTMLElement,
  isCancelled: () => boolean,
): Promise<void> {
  const blocks = Array.from(container.querySelectorAll<HTMLElement>("code.language-mermaid")).filter((code) => {
    const host = code.closest("pre") ?? code;
    return host.getAttribute("data-tc-mermaid-pending") !== "1";
  });
  if (blocks.length === 0) {
    return;
  }
  const mermaid = (await import("mermaid")).default;
  if (isCancelled()) {
    return;
  }
  const dark =
    document.body.classList.contains("vscode-dark")
    || document.body.classList.contains("vscode-high-contrast");
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
    host.setAttribute("data-tc-mermaid-pending", "1");
    try {
      const { svg } = await mermaid.render(id, graph);
      if (isCancelled()) {
        return;
      }
      const figure = document.createElement("figure");
      figure.className = "tc-plan-preview__mermaid";
      figure.setAttribute("data-tc-mermaid-rendered", "1");
      figure.setAttribute("data-testid", "plan-mermaid");
      figure.innerHTML = svg;
      host.replaceWith(figure);
    } catch {
      if (!isCancelled()) {
        host.removeAttribute("data-tc-mermaid-pending");
        host.setAttribute("data-mermaid-error", "1");
      }
    }
  }
}
