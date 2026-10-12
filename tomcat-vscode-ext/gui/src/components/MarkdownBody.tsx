import { memo, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { useT } from "../i18n/LocaleProvider";

import { buildDecoratedHtml, flashCopyButton, setCopyButtonCopiedState } from "./markdown/markdownDecorators";
import { renderMermaidBlocks } from "./markdown/markdownRuntime";
import type { PathResolution } from "../types";

interface MarkdownBodyProps {
  markdown: string;
  onOpenFile?(path: string, line?: number): void;
  onOpenLink(href: string): void;
  resolvePaths?: (paths: string[]) => Promise<PathResolution[]>;
  /** 1-based source file line for each line of `markdown` (see planDocument). */
  sourceLineMap?: number[];
}

function sameSourceLineMap(
  previous: readonly number[] | undefined,
  next: readonly number[] | undefined,
): boolean {
  if (previous === next) {
    return true;
  }
  if (!previous || !next || previous.length !== next.length) {
    return false;
  }
  return previous.every((line, index) => line === next[index]);
}

/**
 * Find decorations are real inline nodes. Unrelated host-state updates must not
 * ask React to reset MarkdownBody's `dangerouslySetInnerHTML`, because doing so
 * detaches those nodes between a Find navigation click and its reveal call.
 */
function areMarkdownBodyPropsEqual(
  previous: Readonly<MarkdownBodyProps>,
  next: Readonly<MarkdownBodyProps>,
): boolean {
  return (
    previous.markdown === next.markdown &&
    previous.onOpenFile === next.onOpenFile &&
    previous.onOpenLink === next.onOpenLink &&
    previous.resolvePaths === next.resolvePaths &&
    sameSourceLineMap(previous.sourceLineMap, next.sourceLineMap)
  );
}

/**
 * Render the plan body markdown as sanitized HTML. Links never navigate the
 * webview directly — clicks are intercepted and forwarded to the host via
 * `onOpenLink`, matching the strict CSP (no inline scripts, no navigation).
 */
export const MarkdownBody = memo(function MarkdownBody({
  markdown,
  onOpenFile,
  onOpenLink,
  resolvePaths,
  sourceLineMap,
}: MarkdownBodyProps) {
  const t = useT();
  const tRef = useRef(t);
  tRef.current = t;
  const containerRef = useRef<HTMLDivElement>(null);
  const [pathResolutions, setPathResolutions] = useState<ReadonlyMap<string, PathResolution>>(
    () => new Map(),
  );
  const html = useMemo(
    () => buildDecoratedHtml(markdown, { pathResolutions, sourceLineMap }),
    [markdown, pathResolutions, sourceLineMap],
  );
  const htmlMarkup = useMemo(() => ({ __html: html }), [html]);
  useEffect(() => {
    for (const button of containerRef.current?.querySelectorAll<HTMLElement>("[data-tc-copy-code]") ?? []) {
      setCopyButtonCopiedState(button, button.classList.contains("is-copied"), t);
    }
    for (const link of containerRef.current?.querySelectorAll<HTMLElement>("[data-tc-default-image-label]") ?? []) {
      link.textContent = t("image.label"); link.title = t("image.label");
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
    let cancelled = false;
    const resolvedFontSize = Number.parseFloat(
      container.ownerDocument.defaultView?.getComputedStyle(container).fontSize ?? "",
    );
    void renderMermaidBlocks(container, () => cancelled, {
      fontSize: Number.isFinite(resolvedFontSize) ? resolvedFontSize : undefined,
    });
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
          () => flashCopyButton(copyButton, (key, args) => tRef.current(key, args)),
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
      dangerouslySetInnerHTML={htmlMarkup}
      onClick={handleClick}
      ref={containerRef}
    />
  );
}, areMarkdownBodyPropsEqual);
