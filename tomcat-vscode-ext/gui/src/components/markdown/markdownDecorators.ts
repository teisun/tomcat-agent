import { fileChipIconClass } from "../FileChip";
import { parseCodeFenceInfo } from "./codeFence";
import { basenameOf, detectInlineFilePath, inferLanguageFromPath } from "./inlinePath";
import { renderMarkdownHtml, sanitizeMarkdownHtml } from "./markdownRuntime";

function withLocationSuffix(label: string, line?: number, column?: number): string {
  if (typeof line !== "number") {
    return label;
  }
  return typeof column === "number" ? `${label}:${line}:${column}` : `${label}:${line}`;
}

function displayFileLabel(path: string, line?: number, column?: number): string {
  return withLocationSuffix(path, line, column);
}

function displayBasenameLabel(path: string, line?: number, column?: number): string {
  return withLocationSuffix(basenameOf(path), line, column);
}

export function normalizeHighlightLanguage(value: string | undefined): string {
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

function createCopyButton(documentRef: Document): HTMLButtonElement {
  const copyButton = documentRef.createElement("button");
  copyButton.className = "tc-code-card__copy";
  copyButton.dataset.tcCopyCode = "1";
  copyButton.dataset.testid = "assistant-code-copy";
  copyButton.type = "button";
  copyButton.title = "Copy code";
  copyButton.setAttribute("aria-label", "Copy code");

  const copyIcon = documentRef.createElement("span");
  copyIcon.setAttribute("aria-hidden", "true");
  copyIcon.className = "tc-code-card__copy-icon codicon codicon-copy";
  copyButton.appendChild(copyIcon);

  return copyButton;
}

export function decorateCodeCards(container: HTMLElement): void {
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
    const copyButton = createCopyButton(documentRef);

    if (parsed.filePath) {
      const header = documentRef.createElement("div");
      header.className = "tc-code-card__header";

      const meta = documentRef.createElement("div");
      meta.className = "tc-code-card__meta";

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
      label.textContent = displayBasenameLabel(parsed.filePath, parsed.line);
      fileButton.appendChild(label);

      meta.appendChild(fileButton);

      header.append(meta, copyButton);
      pre.replaceWith(wrapper);
      wrapper.append(header, pre);
      continue;
    }

    wrapper.classList.add("tc-code-card--bare");
    pre.replaceWith(wrapper);
    wrapper.append(pre, copyButton);
  }
}

export function linkifyInlineFilePaths(container: HTMLElement): void {
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
    link.title = displayFileLabel(match.path, match.line, match.column);
    if (typeof match.line === "number") {
      link.dataset.tcLine = String(match.line);
    }

    const icon = documentRef.createElement("span");
    icon.className = `tc-inline-path__icon codicon ${fileChipIconClass(match.path)}`;
    icon.setAttribute("aria-hidden", "true");
    link.appendChild(icon);

    const label = documentRef.createElement("span");
    label.className = "tc-inline-path__label";
    label.textContent = displayBasenameLabel(match.path, match.line, match.column);
    link.appendChild(label);

    code.replaceWith(link);
  }
}

export function buildDecoratedHtml(markdown: string): string {
  if (typeof document === "undefined") {
    return sanitizeMarkdownHtml(renderMarkdownHtml(markdown));
  }
  const container = document.createElement("div");
  container.innerHTML = sanitizeMarkdownHtml(renderMarkdownHtml(markdown));
  decorateCodeCards(container);
  linkifyInlineFilePaths(container);
  return container.innerHTML;
}
