import { memo, useEffect, useLayoutEffect, useMemo, useRef, type MouseEvent } from "react";

import {
  buildDecoratedHtml,
  normalizeHighlightLanguage,
} from "./markdownDecorators";
import { renderMermaidBlocks } from "./markdownRuntime";
import { getHighlighter, logRichRender } from "./richRenderRuntime";

const STREAMING_ENHANCEMENT_DEBOUNCE_MS = 120;

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

async function highlightCodeBlocks(container: HTMLElement, isCancelled: () => boolean): Promise<void> {
  const codes = Array.from(container.querySelectorAll<HTMLElement>("pre > code")).filter(
    (node) => !node.classList.contains("language-mermaid"),
  );
  if (codes.length === 0) {
    logRichRender("highlight: done", { cancelled: false, done: 0, nodes: 0 });
    return;
  }
  const hljs = await getHighlighter();
  if (isCancelled()) {
    logRichRender("highlight: cancelled", { cancelled: true, done: 0, nodes: codes.length });
    return;
  }
  let highlightedCount = 0;
  for (const code of codes) {
    if (isCancelled()) {
      logRichRender("highlight: cancelled", {
        cancelled: true,
        done: highlightedCount,
        nodes: codes.length,
      });
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
    highlightedCount += 1;
  }
  logRichRender("highlight: done", {
    cancelled: false,
    done: highlightedCount,
    nodes: codes.length,
  });
}

function setCopyButtonCopiedState(button: HTMLElement, copied: boolean): void {
  button.classList.toggle("is-copied", copied);
  button.setAttribute("aria-label", copied ? "Copied" : "Copy code");
  button.title = copied ? "Copied" : "Copy code";
  const icon = button.querySelector<HTMLElement>(".tc-code-card__copy-icon");
  if (!icon) {
    return;
  }
  icon.classList.toggle("codicon-copy", !copied);
  icon.classList.toggle("codicon-check", copied);
}

function flashCopyButton(button: HTMLElement): void {
  const existingTimer = button.dataset.tcCopyResetTimer;
  if (existingTimer) {
    window.clearTimeout(Number(existingTimer));
  }
  setCopyButtonCopiedState(button, true);
  button.dataset.tcCopyResetTimer = String(
    window.setTimeout(() => {
      setCopyButtonCopiedState(button, false);
      delete button.dataset.tcCopyResetTimer;
    }, 1_500),
  );
}

function scheduleEnhancement(isStreaming: boolean, callback: () => void): () => void {
  if (isStreaming) {
    const timeoutId = window.setTimeout(callback, STREAMING_ENHANCEMENT_DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timeoutId);
    };
  }
  let disposed = false;
  Promise.resolve().then(() => {
    if (!disposed) {
      callback();
    }
  });
  return () => {
    disposed = true;
  };
}

function ChatMarkdownComponent({
  isStreaming = false,
  markdown,
  onOpenFile,
  onOpenLink,
}: {
  isStreaming?: boolean;
  markdown: string;
  onOpenFile(path: string, line?: number): void;
  onOpenLink?(href: string): void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const appliedHtmlRef = useRef<string | null>(null);
  const stableMarkdown = useMemo(() => closeOpenFenceIfNeeded(markdown), [markdown]);
  const decoratedHtml = useMemo(() => buildDecoratedHtml(stableMarkdown), [stableMarkdown]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    if (appliedHtmlRef.current === decoratedHtml) {
      logRichRender("innerHTML: skip(same)", { len: decoratedHtml.length });
      return;
    }
    container.innerHTML = decoratedHtml;
    appliedHtmlRef.current = decoratedHtml;
    logRichRender("innerHTML: apply", { len: decoratedHtml.length });
  }, [decoratedHtml]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    logRichRender("effect fire", { len: decoratedHtml.length, streaming: isStreaming });
    let cancelled = false;
    const cancelScheduled = scheduleEnhancement(isStreaming, () => {
      void (async () => {
        try {
          await highlightCodeBlocks(container, () => cancelled);
        } catch (error) {
          logRichRender(
            "highlight: FAILED",
            { error: error instanceof Error ? error.message : String(error) },
            "warn",
          );
        }
        if (cancelled) {
          return;
        }
        try {
          await renderMermaidBlocks(container, () => cancelled);
        } catch (error) {
          logRichRender(
            "mermaid: FAILED",
            { error: error instanceof Error ? error.message : String(error) },
            "warn",
          );
        }
        if (cancelled) {
          return;
        }
        try {
          await highlightCodeBlocks(container, () => cancelled);
        } catch (error) {
          logRichRender(
            "highlight: FAILED",
            { error: error instanceof Error ? error.message : String(error) },
            "warn",
          );
        }
      })();
    });
    return () => {
      cancelled = true;
      cancelScheduled();
    };
  }, [decoratedHtml, isStreaming]);

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
      onClick={handleClick}
      ref={containerRef}
    />
  );
}

export const ChatMarkdown = memo(ChatMarkdownComponent);
