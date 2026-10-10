import { memo, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { useT } from "../../i18n/LocaleProvider";

import { buildDecoratedHtml, flashCopyButton, setCopyButtonCopiedState } from "./markdownDecorators";
import { renderMermaidBlocks, splitTopLevelBlocks } from "./markdownRuntime";
import { logRichRender } from "./richRenderRuntime";
import type { PathResolution, WebviewMediaRoot } from "../../types";

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

const ChatMarkdownBlock = memo(function ChatMarkdownBlock({
  mediaRoots,
  raw,
  resolvePaths,
}: {
  mediaRoots?: WebviewMediaRoot[];
  raw: string;
  resolvePaths?: (paths: string[]) => Promise<PathResolution[]>;
}) {
  const t = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const [pathResolutions, setPathResolutions] = useState<ReadonlyMap<string, PathResolution>>(
    () => new Map(),
  );
  const html = useMemo(
    () =>
      buildDecoratedHtml(closeOpenFenceIfNeeded(raw), {
        mediaRoots,
        pathResolutions,
      }),
    [mediaRoots, pathResolutions, raw],
  );
  // Keep the HTML wrapper stable: a locale-only render must not replace live code/diagram nodes.
  const htmlMarkup = useMemo(() => ({ __html: html }), [html]);

  useEffect(() => {
    for (const button of containerRef.current?.querySelectorAll<HTMLElement>("[data-tc-copy-code]") ?? []) {
      setCopyButtonCopiedState(button, button.classList.contains("is-copied"), t);
    }
    for (const link of containerRef.current?.querySelectorAll<HTMLElement>("[data-tc-default-image-label]") ?? []) {
      link.textContent = t("image.label");
      link.title = t("image.label");
    }
  }, [html, t]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !resolvePaths) {
      return;
    }
    const paths = [
      ...new Set(
        Array.from(container.querySelectorAll<HTMLElement>("[data-tc-path-candidate]"))
          .map((node) => node.dataset.tcPathCandidate)
          .filter((path): path is string => Boolean(path)),
      ),
    ];
    if (paths.length === 0) {
      return;
    }
    let cancelled = false;
    void resolvePaths(paths).then(
      (results) => {
        if (cancelled) {
          return;
        }
        setPathResolutions((current) => {
          const next = new Map(current);
          for (const result of results) {
            next.set(result.path, result);
          }
          return next;
        });
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [html, resolvePaths]);

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
      dangerouslySetInnerHTML={htmlMarkup}
      ref={containerRef}
    />
  );
});

function ChatMarkdownComponent({
  markdown,
  mediaRoots,
  onOpenFile,
  onOpenLink,
  resolvePaths,
  onZoomImage,
}: {
  markdown: string;
  mediaRoots?: WebviewMediaRoot[];
  onOpenFile(path: string, line?: number): void;
  onOpenLink?(href: string): void;
  resolvePaths?: (paths: string[]) => Promise<PathResolution[]>;
  onZoomImage?(image: { alt: string; src: string }): void;
}) {
  const t = useT();
  const tRef = useRef(t);
  tRef.current = t;
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
          () => flashCopyButton(copyButton, (key, args) => tRef.current(key, args)),
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

    const imageTarget = target?.closest<HTMLElement>("[data-tc-image-src]");
    if (imageTarget) {
      event.preventDefault();
      event.stopPropagation();
      const image = imageTarget as HTMLImageElement;
      const src = image.dataset.tcImageSrc ?? image.getAttribute("src") ?? "";
      if (src) {
        onZoomImage?.({
          alt: image.getAttribute("alt") ?? "",
          src,
        });
      }
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
      {blocks.map((raw, index) => (
        <ChatMarkdownBlock
          key={index}
          mediaRoots={mediaRoots}
          raw={raw}
          resolvePaths={resolvePaths}
        />
      ))}
    </div>
  );
}

export const ChatMarkdown = memo(ChatMarkdownComponent);
