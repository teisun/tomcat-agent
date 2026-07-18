import { useEffect, useMemo, useRef, type MouseEvent } from "react";

import { fileChipIconClass } from "../FileChip";
import { parseCodeFenceInfo } from "./codeFence";
import { detectInlineFilePath, inferLanguageFromPath } from "./inlinePath";
import {
  renderMarkdownHtml,
  renderMermaidBlocks,
  sanitizeMarkdownHtml,
} from "./markdownRuntime";

type HighlightJsCore = typeof import("highlight.js/lib/core");

let highlighterPromise: Promise<HighlightJsCore["default"]> | null = null;

function displayFileLabel(path: string, line?: number): string {
  return typeof line === "number" ? `${path}:${line}` : path;
}

function normalizeHighlightLanguage(value: string | undefined): string {
  switch (value?.toLowerCase()) {
    case undefined:
      return "plaintext";
    case "cjs":
    case "js":
    case "jsx":
    case "mjs":
      return "javascript";
    case "html":
      return "xml";
    case "md":
      return "markdown";
    case "py":
      return "python";
    case "rb":
      return "ruby";
    case "rs":
      return "rust";
    case "sh":
    case "shell":
    case "zsh":
      return "bash";
    case "text":
    case "txt":
      return "plaintext";
    case "ts":
    case "tsx":
      return "typescript";
    case "yml":
      return "yaml";
    default:
      return value.toLowerCase();
  }
}

async function getHighlighter(): Promise<HighlightJsCore["default"]> {
  if (!highlighterPromise) {
    highlighterPromise = (async () => {
      const [
        { default: hljs },
        { default: bash },
        { default: c },
        { default: cpp },
        { default: css },
        { default: diff },
        { default: go },
        { default: ini },
        { default: java },
        { default: javascript },
        { default: json },
        { default: kotlin },
        { default: markdown },
        { default: php },
        { default: plaintext },
        { default: python },
        { default: ruby },
        { default: rust },
        { default: scala },
        { default: sql },
        { default: swift },
        { default: typescript },
        { default: xml },
        { default: yaml },
      ] = await Promise.all([
        import("highlight.js/lib/core"),
        import("highlight.js/lib/languages/bash"),
        import("highlight.js/lib/languages/c"),
        import("highlight.js/lib/languages/cpp"),
        import("highlight.js/lib/languages/css"),
        import("highlight.js/lib/languages/diff"),
        import("highlight.js/lib/languages/go"),
        import("highlight.js/lib/languages/ini"),
        import("highlight.js/lib/languages/java"),
        import("highlight.js/lib/languages/javascript"),
        import("highlight.js/lib/languages/json"),
        import("highlight.js/lib/languages/kotlin"),
        import("highlight.js/lib/languages/markdown"),
        import("highlight.js/lib/languages/php"),
        import("highlight.js/lib/languages/plaintext"),
        import("highlight.js/lib/languages/python"),
        import("highlight.js/lib/languages/ruby"),
        import("highlight.js/lib/languages/rust"),
        import("highlight.js/lib/languages/scala"),
        import("highlight.js/lib/languages/sql"),
        import("highlight.js/lib/languages/swift"),
        import("highlight.js/lib/languages/typescript"),
        import("highlight.js/lib/languages/xml"),
        import("highlight.js/lib/languages/yaml"),
      ]);

      hljs.registerLanguage("bash", bash);
      hljs.registerLanguage("c", c);
      hljs.registerLanguage("cpp", cpp);
      hljs.registerLanguage("css", css);
      hljs.registerLanguage("diff", diff);
      hljs.registerLanguage("go", go);
      hljs.registerLanguage("ini", ini);
      hljs.registerLanguage("java", java);
      hljs.registerLanguage("javascript", javascript);
      hljs.registerLanguage("json", json);
      hljs.registerLanguage("kotlin", kotlin);
      hljs.registerLanguage("markdown", markdown);
      hljs.registerLanguage("php", php);
      hljs.registerLanguage("plaintext", plaintext);
      hljs.registerLanguage("python", python);
      hljs.registerLanguage("ruby", ruby);
      hljs.registerLanguage("rust", rust);
      hljs.registerLanguage("scala", scala);
      hljs.registerLanguage("sql", sql);
      hljs.registerLanguage("swift", swift);
      hljs.registerLanguage("typescript", typescript);
      hljs.registerLanguage("xml", xml);
      hljs.registerLanguage("yaml", yaml);
      hljs.registerAliases?.(["sh", "shell", "zsh"], { languageName: "bash" });
      hljs.registerAliases?.(["js", "jsx", "mjs", "cjs"], { languageName: "javascript" });
      hljs.registerAliases?.(["ts", "tsx"], { languageName: "typescript" });
      hljs.registerAliases?.(["html"], { languageName: "xml" });
      hljs.registerAliases?.(["md"], { languageName: "markdown" });
      hljs.registerAliases?.(["py"], { languageName: "python" });
      hljs.registerAliases?.(["rb"], { languageName: "ruby" });
      hljs.registerAliases?.(["rs"], { languageName: "rust" });
      hljs.registerAliases?.(["text", "txt"], { languageName: "plaintext" });
      hljs.registerAliases?.(["yml"], { languageName: "yaml" });
      return hljs;
    })();
  }
  return highlighterPromise;
}

function closeOpenFenceIfNeeded(markdown: string): string {
  const lines = markdown.split("\n");
  const fenceStack: string[] = [];
  for (const line of lines) {
    const match = line.match(/^[ \t]*(`{3,}|~{3,})/u);
    if (!match) {
      continue;
    }
    const fence = match[1];
    const current = fenceStack.at(-1);
    if (current === fence[0]) {
      fenceStack.pop();
    } else {
      fenceStack.push(fence[0]);
    }
  }
  if (fenceStack.length === 0) {
    return markdown;
  }
  return `${markdown}\n${fenceStack.map((char) => char.repeat(3)).join("\n")}`;
}

function decorateCodeCards(container: HTMLElement): void {
  const documentRef = container.ownerDocument;
  const blocks = Array.from(container.querySelectorAll<HTMLPreElement>("pre")).filter(
    (pre) => !pre.closest(".tc-code-card"),
  );
  for (const pre of blocks) {
    const code = pre.querySelector("code");
    if (!code || code.classList.contains("language-mermaid")) {
      continue;
    }
    const parsed = parseCodeFenceInfo(pre.getAttribute("data-fence-info"));
    if (parsed.isMermaid) {
      continue;
    }

    const language = normalizeHighlightLanguage(parsed.language ?? inferLanguageFromPath(parsed.filePath ?? ""));
    code.classList.add(`language-${language}`);

    const wrapper = documentRef.createElement("section");
    wrapper.className = "tc-code-card";
    wrapper.setAttribute("data-testid", "assistant-code-card");

    const header = documentRef.createElement("div");
    header.className = "tc-code-card__header";

    const meta = documentRef.createElement("div");
    meta.className = "tc-code-card__meta";

    const lang = documentRef.createElement("span");
    lang.className = "tc-code-card__lang";
    lang.textContent = parsed.languageLabel;
    meta.appendChild(lang);

    if (parsed.filePath) {
      const fileButton = documentRef.createElement("button");
      fileButton.className = "tc-code-card__file";
      fileButton.dataset.tcFilePath = parsed.filePath;
      fileButton.dataset.testid = "assistant-code-file";
      fileButton.type = "button";
      if (typeof parsed.line === "number") {
        fileButton.dataset.tcLine = String(parsed.line);
      }
      fileButton.title = displayFileLabel(parsed.filePath, parsed.line);

      const icon = documentRef.createElement("span");
      icon.className = `tc-code-card__file-icon codicon ${fileChipIconClass(parsed.filePath)}`;
      icon.setAttribute("aria-hidden", "true");
      fileButton.appendChild(icon);

      const label = documentRef.createElement("span");
      label.className = "tc-code-card__file-label";
      label.textContent = displayFileLabel(parsed.filePath, parsed.line);
      fileButton.appendChild(label);

      meta.appendChild(fileButton);
    }

    const copyButton = documentRef.createElement("button");
    copyButton.className = "tc-code-card__copy";
    copyButton.dataset.tcCopyCode = "1";
    copyButton.dataset.testid = "assistant-code-copy";
    copyButton.type = "button";
    const copyIcon = documentRef.createElement("span");
    copyIcon.setAttribute("aria-hidden", "true");
    copyIcon.className = "codicon codicon-copy";
    const copyLabel = documentRef.createElement("span");
    copyLabel.textContent = "Copy";
    copyButton.append(copyIcon, copyLabel);

    header.append(meta, copyButton);
    pre.replaceWith(wrapper);
    wrapper.append(header, pre);
  }
}

async function highlightCodeBlocks(container: HTMLElement, isCancelled: () => boolean): Promise<void> {
  const codes = Array.from(container.querySelectorAll<HTMLElement>("pre > code")).filter(
    (node) => !node.classList.contains("language-mermaid"),
  );
  if (codes.length === 0) {
    return;
  }
  const hljs = await getHighlighter();
  if (isCancelled()) {
    return;
  }
  for (const code of codes) {
    if (isCancelled()) {
      return;
    }
    if (code.dataset.tcHighlighted === "1") {
      continue;
    }
    const explicitClass = [...code.classList].find((className) => className.startsWith("language-"));
    const explicitLanguage = explicitClass?.slice("language-".length);
    const normalizedLanguage = normalizeHighlightLanguage(explicitLanguage ?? undefined);
    const rawText = code.textContent ?? "";
    const language = hljs.getLanguage(normalizedLanguage) ? normalizedLanguage : "plaintext";
    code.classList.remove(...[...code.classList].filter((className) => className.startsWith("language-")));
    code.classList.add("hljs", `language-${language}`);
    code.innerHTML = hljs.highlight(rawText, { ignoreIllegals: true, language }).value;
    code.dataset.tcHighlighted = "1";
  }
}

function linkifyInlineFilePaths(container: HTMLElement): void {
  const documentRef = container.ownerDocument;
  const inlineCodes = Array.from(container.querySelectorAll<HTMLElement>("code")).filter(
    (node) => !node.closest("pre"),
  );
  for (const code of inlineCodes) {
    const text = code.textContent ?? "";
    const match = detectInlineFilePath(text);
    if (!match) {
      continue;
    }
    const link = documentRef.createElement("a");
    link.className = "tc-inline-path";
    link.dataset.tcFilePath = match.path;
    link.dataset.testid = "assistant-clickable-path";
    link.href = "#";
    link.title = displayFileLabel(match.path, match.line);
    if (typeof match.line === "number") {
      link.dataset.tcLine = String(match.line);
    }

    const icon = documentRef.createElement("span");
    icon.className = `tc-inline-path__icon codicon ${fileChipIconClass(match.path)}`;
    icon.setAttribute("aria-hidden", "true");
    link.appendChild(icon);

    const label = documentRef.createElement("span");
    label.className = "tc-inline-path__label";
    label.textContent = match.originalText;
    link.appendChild(label);

    code.replaceWith(link);
  }
}

function buildDecoratedHtml(markdown: string): string {
  if (typeof document === "undefined") {
    return sanitizeMarkdownHtml(renderMarkdownHtml(markdown));
  }
  const container = document.createElement("div");
  container.innerHTML = sanitizeMarkdownHtml(renderMarkdownHtml(markdown));
  decorateCodeCards(container);
  linkifyInlineFilePaths(container);
  return container.innerHTML;
}

export function ChatMarkdown({
  markdown,
  onOpenFile,
  onOpenLink,
}: {
  markdown: string;
  onOpenFile(path: string, line?: number): void;
  onOpenLink?(href: string): void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const stableMarkdown = useMemo(() => closeOpenFenceIfNeeded(markdown), [markdown]);
  const decoratedHtml = useMemo(() => buildDecoratedHtml(stableMarkdown), [stableMarkdown]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    let cancelled = false;
    void (async () => {
      await renderMermaidBlocks(container, () => cancelled);
      if (cancelled) {
        return;
      }
      try {
        await highlightCodeBlocks(container, () => cancelled);
      } catch {
        // Rich rendering should degrade to plain code, not block clickable paths.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [decoratedHtml]);

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement | null;
    const copyButton = target?.closest<HTMLElement>("[data-tc-copy-code]");
    if (copyButton) {
      event.preventDefault();
      event.stopPropagation();
      const card = copyButton.closest(".tc-code-card");
      const codeText = card?.querySelector("pre code")?.textContent ?? "";
      if (typeof navigator?.clipboard?.writeText === "function") {
        void navigator.clipboard.writeText(codeText);
      }
      return;
    }

    const fileTarget = target?.closest<HTMLElement>("[data-tc-file-path]");
    if (fileTarget) {
      event.preventDefault();
      event.stopPropagation();
      const line = fileTarget.dataset.tcLine ? Number(fileTarget.dataset.tcLine) : undefined;
      onOpenFile(fileTarget.dataset.tcFilePath ?? "", Number.isFinite(line) ? line : undefined);
      return;
    }

    const anchor = target?.closest<HTMLAnchorElement>("a");
    if (!anchor) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const href = anchor.getAttribute("href");
    if (href && onOpenLink) {
      onOpenLink(href);
    }
  };

  return (
    <div
      className="rendered-markdown tc-chat-markdown"
      data-testid="chat-markdown"
      dangerouslySetInnerHTML={{ __html: decoratedHtml }}
      onClick={handleClick}
      ref={containerRef}
    />
  );
}
