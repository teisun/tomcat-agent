import { memo, useEffect, useMemo, useRef, type MouseEvent } from "react";

import { buildDecoratedHtml, flashCopyButton } from "./markdownDecorators";
import { renderMermaidBlocks, splitTopLevelBlocks } from "./markdownRuntime";
import { logRichRender } from "./richRenderRuntime";

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

const ChatMarkdownBlock = memo(function ChatMarkdownBlock({ raw }: { raw: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const html = useMemo(() => buildDecoratedHtml(closeOpenFenceIfNeeded(raw)), [raw]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    logRichRender("block: mermaid effect", { htmlLength: html.length });
    let cancelled = false;
    void renderMermaidBlocks(container, () => cancelled).catch((error) => {
      logRichRender(
        "mermaid: FAILED",
        { error: error instanceof Error ? error.message : String(error) },
        "warn",
      );
    });
    return () => {
      cancelled = true;
    };
  }, [html]);

  return (
    <div
      className="tc-chat-markdown__block"
      dangerouslySetInnerHTML={{ __html: html }}
      ref={containerRef}
    />
  );
});

function ChatMarkdownComponent({
  markdown,
  onOpenFile,
  onOpenLink,
}: {
  markdown: string;
  onOpenFile(path: string, line?: number): void;
  onOpenLink?(href: string): void;
}) {
  const blocks = useMemo(() => splitTopLevelBlocks(markdown), [markdown]);

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
    >
      {blocks.map((raw, index) => <ChatMarkdownBlock key={index} raw={raw} />)}
    </div>
  );
}

export const ChatMarkdown = memo(ChatMarkdownComponent);
