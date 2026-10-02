import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  isPlanPreviewHostFrame,
  type PlanPreviewDomAction,
  type PlanPreviewDomSnapshot,
  type PlanPreviewIntent,
  type PlanPreviewStateSnapshot,
  type PlanTodoStatus,
  type PlanToolbarStyle,
  type VsCodeApiLike,
} from "../../../src/shared/planPreviewProtocol";
import type { PathResolution } from "../../../src/shared/pathResolution";
import { MarkdownBody } from "../components/MarkdownBody";
import { PlanActionStrip } from "../components/PlanActionStrip";
import { TodoList } from "../components/TodoList";
import { PlanFindWidget } from "./PlanFindWidget";
import { usePlanFind } from "./usePlanFind";
import { PlanSelectionActionButton } from "./PlanSelectionActionButton";

type DistributiveOmit<T, K extends keyof T> = T extends unknown
  ? Omit<T, K>
  : never;
type PlanIntentWithoutId = DistributiveOmit<PlanPreviewIntent, "messageId">;

function send(
  vscodeApi: VsCodeApiLike<PlanPreviewIntent>,
  message: PlanIntentWithoutId,
): void {
  vscodeApi.postMessage({
    ...message,
    messageId: `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  } as PlanPreviewIntent);
}

function todoCountLabel(count: number): string {
  return `${count} ${count === 1 ? "To-do" : "To-dos"}`;
}

/** 1-based source line for a DOM node via the nearest `[data-source-line]` block. */
function sourceLineOf(node: Node | null): number | null {
  const element =
    node instanceof Element ? node : (node?.parentElement ?? null);
  const host = element?.closest("[data-source-line]") ?? null;
  const raw = host?.getAttribute("data-source-line");
  if (!raw) {
    return null;
  }
  const line = Number.parseInt(raw, 10);
  return Number.isFinite(line) ? line : null;
}

/**
 * 1-based inclusive source line range of the live selection, read from the
 * `data-source-line` attributes MarkdownBody stamps on each rendered block.
 * Unlike matching rendered text back to the raw source, this is exact and
 * unaffected by inline markdown. Returns null when the selection is not inside a
 * source-mapped block (e.g. the todo checklist), so the caller omits line info.
 */
function readSelectionSourceLines(): {
  lineEnd: number;
  lineStart: number;
} | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) {
    return null;
  }
  const range = selection.getRangeAt(0);
  const startLine = sourceLineOf(range.startContainer);
  const endLine = sourceLineOf(range.endContainer);
  const first = startLine ?? endLine;
  const last = endLine ?? startLine;
  if (first == null || last == null) {
    return null;
  }
  return { lineEnd: Math.max(first, last), lineStart: Math.min(first, last) };
}

type ScrollRestoreState = {
  anchorLine: number | null;
  anchorOffset: number;
  anchorTagName: string | null;
  anchorText: string;
  scrollTop: number;
};

function normalizeAnchorText(text: string): string {
  return text.replace(/\s+/gu, " ").trim().slice(0, 160);
}

function findFirstVisibleSourceBlock(
  container: HTMLElement,
): HTMLElement | null {
  const containerRect = container.getBoundingClientRect();
  const blocks = Array.from(
    container.querySelectorAll<HTMLElement>("[data-source-line]"),
  );
  return (
    blocks.find((block) => {
      const rect = block.getBoundingClientRect();
      return rect.bottom > containerRect.top && rect.top < containerRect.bottom;
    }) ??
    blocks[0] ??
    null
  );
}

function captureScrollRestore(
  container: HTMLElement | null,
): ScrollRestoreState | null {
  if (!container) {
    return null;
  }
  const anchor = findFirstVisibleSourceBlock(container);
  if (!anchor) {
    return {
      anchorLine: null,
      anchorOffset: 0,
      anchorTagName: null,
      anchorText: "",
      scrollTop: container.scrollTop,
    };
  }
  const containerRect = container.getBoundingClientRect();
  return {
    anchorLine: sourceLineOf(anchor),
    anchorOffset: anchor.getBoundingClientRect().top - containerRect.top,
    anchorTagName: anchor.tagName,
    anchorText: normalizeAnchorText(anchor.textContent ?? ""),
    scrollTop: container.scrollTop,
  };
}

function findAnchorByText(
  container: HTMLElement,
  tagName: string | null,
  anchorText: string,
): HTMLElement | null {
  if (!tagName || !anchorText) {
    return null;
  }
  return (
    Array.from(
      container.querySelectorAll<HTMLElement>("[data-source-line]"),
    ).find(
      (candidate) =>
        candidate.tagName === tagName &&
        normalizeAnchorText(candidate.textContent ?? "").startsWith(anchorText),
    ) ?? null
  );
}

function restoreScrollPosition(
  container: HTMLElement,
  restore: ScrollRestoreState,
): void {
  let anchor: HTMLElement | null = null;
  if (restore.anchorLine !== null) {
    anchor = container.querySelector<HTMLElement>(
      `[data-source-line="${restore.anchorLine}"]`,
    );
  }
  if (!anchor) {
    anchor = findAnchorByText(
      container,
      restore.anchorTagName,
      restore.anchorText,
    );
  }
  if (!anchor) {
    container.scrollTop = restore.scrollTop;
    return;
  }
  const containerRect = container.getBoundingClientRect();
  const absoluteTop =
    anchor.getBoundingClientRect().top -
    containerRect.top +
    container.scrollTop;
  container.scrollTop = Math.max(0, absoluteTop - restore.anchorOffset);
}

const PLAN_PREVIEW_WEBVIEW_INSTANCE_ID = `plan-webview-${Date.now()}-${Math.random().toString(36).slice(2)}`;

function computedFontSize(element: Element | null): number | null {
  if (!element) {
    return null;
  }
  const value = Number.parseFloat(
    element.ownerDocument.defaultView?.getComputedStyle(element).fontSize ?? "",
  );
  return Number.isFinite(value) ? value : null;
}

/** Read the rendered DOM for E2E assertions (test-only). */
function readDomSnapshot(
  state: PlanPreviewStateSnapshot | null,
  webviewStateFrames: number,
): PlanPreviewDomSnapshot {
  const strip = document.querySelector('[data-testid="plan-action-strip"]');
  const modelTrigger = document.querySelector<HTMLElement>(
    '[data-testid="plan-build-model-select"]',
  );
  const countEl = document.querySelector('[data-testid="plan-todos-count"]');
  const items = document.querySelectorAll('[data-testid="plan-todo-item"]');
  const icons = document.querySelectorAll(".tc-plan-todo__icon");
  const body = document.querySelector('[data-testid="plan-markdown-body"]');
  const code = body?.querySelector("code") ?? null;
  const content = document.querySelector<HTMLElement>(
    '[data-testid="plan-content"]',
  );
  const find = document.querySelector('[data-testid="plan-find"]');
  const findCount = document.querySelector('[data-testid="plan-find-count"]');
  const mermaidText = document.querySelector(
    '[data-testid="plan-mermaid"] svg text, [data-testid="plan-mermaid"] svg foreignObject span, [data-testid="plan-mermaid"] .nodeLabel',
  );
  const todoContent = document.querySelector(".tc-plan-todo__content");
  const inlinePathCount = document.querySelectorAll(".tc-inline-path").length;
  const options = state?.availableModels ?? [];
  const buildModelValue = state?.buildModel || state?.sessionModel || "";
  const buildModelLabel = modelTrigger?.textContent?.trim() ?? buildModelValue;
  const todoIconSizes = Array.from(icons).map((icon) =>
    Math.round(icon.getBoundingClientRect().width),
  );
  const todoStatuses: PlanTodoStatus[] = Array.from(
    items,
  ).flatMap<PlanTodoStatus>((item) => {
    const status = item.getAttribute("data-status");
    return status === "cancelled" ||
      status === "completed" ||
      status === "in_progress" ||
      status === "pending"
      ? [status]
      : [];
  });
  const toolbarStyle: PlanToolbarStyle = state?.toolbarStyle ?? "native";
  const mermaidSvgCount = document.querySelectorAll(
    '[data-testid="plan-mermaid"] svg',
  ).length;
  // The fixed action strip must sit outside the scrolling content column so it
  // never scrolls away; assert the structural invariant here for E2E.
  const stripOutsideContent = Boolean(
    strip && content && !content.contains(strip),
  );
  // Left inset of the strip: ~0 confirms the full-bleed header (no leftover VS
  // Code body padding). null when the strip isn't rendered (native toolbar mode).
  const stripInsetLeft = strip
    ? Math.round(strip.getBoundingClientRect().left)
    : null;
  const topVisibleSourceLine = sourceLineOf(
    content ? findFirstVisibleSourceBlock(content) : null,
  );
  return {
    baseFontSizePx: computedFontSize(document.body),
    bodyFontSizePx: computedFontSize(body),
    bodyHasContent: Boolean(body && (body.textContent ?? "").trim().length > 0),
    bodyText: body?.textContent ?? "",
    bodyInsetLeft: body ? Math.round(body.getBoundingClientRect().left) : null,
    buildModelLabel,
    buildModelOptions: options,
    buildModelValue,
    codeFontSizePx: computedFontSize(code),
    findCountText: findCount ? findCount.textContent : null,
    findVisible: Boolean(find),
    contentScrollTop: content ? Math.round(content.scrollTop) : null,
    hasActionStrip: Boolean(strip),
    inlinePathCount,
    mermaidFontSizePx: computedFontSize(mermaidText),
    mermaidSvgCount,
    selectionButtonVisible: Boolean(
      document.querySelector('[data-testid="plan-selection-add"]'),
    ),
    stripInsetLeft,
    stripOutsideContent,
    todoCountText: countEl ? countEl.textContent : null,
    todoFontSizePx: computedFontSize(todoContent),
    todoIconSizes,
    todoItemCount: items.length,
    todoStatuses,
    themeClassName: document.body.className,
    topVisibleSourceLine,
    toolbarStyle,
    webviewInstanceId: PLAN_PREVIEW_WEBVIEW_INSTANCE_ID,
    refreshCounters: {
      hostPostAttempts: 0,
      hostPostDeliveries: 0,
      hostRefreshCalls: 0,
      webviewStateFrames,
    },
  };
}

function runDomAction(action: PlanPreviewDomAction): void {
  switch (action.kind) {
    case "clickBuild":
      document
        .querySelector<HTMLButtonElement>('[data-testid="plan-build"]')
        ?.click();
      return;
    case "clickSelectionAdd":
      document
        .querySelector<HTMLButtonElement>('[data-testid="plan-selection-add"]')
        ?.click();
      return;
    case "clickSelector":
      document.querySelector<HTMLElement>(action.selector)?.click();
      return;
    case "setContentScrollTop": {
      const content = document.querySelector<HTMLElement>(
        '[data-testid="plan-content"]',
      );
      if (content) {
        content.scrollTop = action.scrollTop;
        content.dispatchEvent(new Event("scroll", { bubbles: true }));
      }
      return;
    }
    case "selectText": {
      const target = document.querySelector(action.selector);
      const selection = window.getSelection();
      if (target && selection) {
        const range = document.createRange();
        range.selectNodeContents(target);
        selection.removeAllRanges();
        selection.addRange(range);
      }
      return;
    }
    case "selectBuildModel": {
      openModelPicker(() => {
        findModelOption(action.modelId)
          ?.querySelector<HTMLButtonElement>("button")
          ?.click();
      });
      return;
    }
    case "selectContextWindow": {
      openModelConfig(action.modelId, () => {
        Array.from(
          document.querySelectorAll<HTMLButtonElement>(
            '[data-testid="context-window-option"]',
          ),
        )
          .find((option) =>
            option.textContent
              ?.replaceAll(",", "")
              .includes(String(action.contextWindow)),
          )
          ?.click();
      });
      return;
    }
    case "selectSpeed": {
      openModelConfig(action.modelId, () => {
        Array.from(document.querySelectorAll<HTMLButtonElement>('[data-testid="speed-option"]'))
          .find((option) => option.textContent?.trim().toLowerCase() === action.speed)?.click();
      });
      return;
    }
    case "selectThinkingLevel": {
      openModelConfig(action.modelId, () => {
        Array.from(
          document.querySelectorAll<HTMLButtonElement>(
            '[data-testid="thinking-level-option"]',
          ),
        )
          .find(
            (option) =>
              option.textContent?.trim().toLowerCase() ===
              action.level.toLowerCase(),
          )
          ?.click();
      });
      return;
    }
  }
}

function findModelOption(modelId: string): HTMLElement | null {
  return (
    Array.from(
      document.querySelectorAll<HTMLElement>(".tc-model-picker-option"),
    ).find((option) => option.dataset.modelId === modelId) ?? null
  );
}

function openModelPicker(afterOpen?: () => void): void {
  document
    .querySelector<HTMLElement>('[data-testid="plan-build-model-select"]')
    ?.click();
  if (afterOpen) {
    window.setTimeout(afterOpen, 0);
  }
}

function openModelConfig(modelId: string, afterOpen?: () => void): void {
  openModelPicker(() => {
    const option = findModelOption(modelId);
    option?.querySelector<HTMLButtonElement>(".tc-model-picker-edit")?.click();
    if (afterOpen) {
      window.setTimeout(afterOpen, 0);
    }
  });
}

export function PlanPreviewApp({
  vscodeApi,
}: {
  vscodeApi: VsCodeApiLike<PlanPreviewIntent>;
}) {
  const [state, setState] = useState<PlanPreviewStateSnapshot | null>(null);
  const stateRef = useRef<PlanPreviewStateSnapshot | null>(state);
  const contentRef = useRef<HTMLElement>(null);
  const didFocusPlanRef = useRef(false);
  const pendingScrollRestoreRef = useRef<ScrollRestoreState | null>(null);
  const stateFrameCountRef = useRef(0);
  const pendingPathResolutionsRef = useRef(
    new Map<string, { resolve(results: PathResolution[]): void }>(),
  );
  stateRef.current = state;
  const findContentVersion = useMemo(
    () =>
      JSON.stringify({
        bodyMarkdown: state?.bodyMarkdown ?? "",
        todos:
          state?.todos.map((todo) => (todo.content.length > 0 ? todo.content : todo.id)) ??
          [],
      }),
    [state?.bodyMarkdown, state?.todos],
  );
  const find = usePlanFind({
    contentRef,
    contentVersion: findContentVersion,
  });

  // Once the first state arrives, put focus inside the webview. This is the
  // activation handoff that lets Cmd/Ctrl+F reach this document instead of
  // remaining in VS Code chrome after the custom editor opens.
  useEffect(() => {
    if (!state || didFocusPlanRef.current) {
      return;
    }
    didFocusPlanRef.current = true;
    contentRef.current?.focus({ preventScroll: true });
  }, [state]);

  const sendSelection = useCallback(
    (selectedText: string) => {
      const trimmed = selectedText.trim();
      if (!trimmed) {
        return;
      }
      const lines = readSelectionSourceLines();
      send(vscodeApi, {
        data: lines
          ? {
              lineEnd: lines.lineEnd,
              lineStart: lines.lineStart,
              text: trimmed,
            }
          : { text: trimmed },
        type: "addSelectionToChat",
      });
    },
    [vscodeApi],
  );

  const resolvePaths = useCallback(
    (paths: string[]): Promise<PathResolution[]> => {
      const uniquePaths = [
        ...new Set(paths.map((path) => path.trim()).filter(Boolean)),
      ];
      if (uniquePaths.length === 0) {
        return Promise.resolve([]);
      }
      const requestId = `resolve-paths-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      return new Promise<PathResolution[]>((resolve) => {
        pendingPathResolutionsRef.current.set(requestId, { resolve });
        send(vscodeApi, {
          data: { paths: uniquePaths, requestId },
          type: "resolvePaths",
        });
      });
    },
    [vscodeApi],
  );

  const openPlanFile = useCallback(
    (path: string, line?: number) =>
      send(vscodeApi, { data: { line, path }, type: "openFile" }),
    [vscodeApi],
  );
  const openPlanLink = useCallback(
    (href: string) => send(vscodeApi, { data: { href }, type: "openLink" }),
    [vscodeApi],
  );

  useEffect(() => {
    const handleMessage = (event: MessageEvent<unknown>) => {
      const frame = event.data;
      if (!isPlanPreviewHostFrame(frame)) {
        return;
      }
      if (frame.channel === "state") {
        stateFrameCountRef.current += 1;
        pendingScrollRestoreRef.current = stateRef.current
          ? captureScrollRestore(contentRef.current)
          : null;
        setState(frame.content);
        return;
      }
      if (frame.content.type === "pathsResolved") {
        const pending = pendingPathResolutionsRef.current.get(
          frame.content.requestId,
        );
        if (pending) {
          pendingPathResolutionsRef.current.delete(frame.content.requestId);
          pending.resolve(frame.content.results);
        }
        return;
      }
      if (frame.content.type === "captureSelectionForChat") {
        const selection = window.getSelection();
        sendSelection(selection ? selection.toString() : "");
        return;
      }
      if (frame.content.type === "__test.capture_dom") {
        vscodeApi.postMessage({
          data: readDomSnapshot(stateRef.current, stateFrameCountRef.current),
          messageId: frame.messageId,
          type: "__test.dom_snapshot",
        } as unknown as PlanPreviewIntent);
        return;
      }
      if (frame.content.type === "__test.dom_action") {
        if (frame.content.action.kind === "setFindQuery") {
          find.openFind();
          find.setFindQuery(frame.content.action.query);
        } else {
          runDomAction(frame.content.action);
        }
      }
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, { type: "plan.ready" });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [find.openFind, find.setFindQuery, sendSelection, vscodeApi]);

  useLayoutEffect(() => {
    const container = contentRef.current;
    const restore = pendingScrollRestoreRef.current;
    if (!container || !restore) {
      return;
    }
    pendingScrollRestoreRef.current = null;
    restoreScrollPosition(container, restore);
  }, [state]);

  if (!state) {

    return (
      <div
        className="tc-plan-preview tc-plan-preview--loading"
        data-testid="plan-loading"
      >
        Loading plan…
      </div>
    );
  }

  const isHybrid = state.toolbarStyle === "hybrid";
  const sessionModelDetails = state.availableModelDetails[state.sessionModel];

  return (
    <div
      className={`tc-plan-preview${find.open ? " tc-plan-preview--find-open" : ""}`}
    >
      {isHybrid ? (
        <PlanActionStrip
          availableModelDetails={state.availableModelDetails}
          availableModels={state.availableModels}
          buildModel={state.buildModel}
          canBuild={state.canBuild}
          fileState={state.state}
          onBuild={() => send(vscodeApi, { type: "build" })}
          onSetBuildModel={(modelId) =>
            send(vscodeApi, { data: { modelId }, type: "setBuildModel" })
          }
          onSelectContextWindow={(modelId, contextWindow) =>
            send(vscodeApi, {
              data: { contextWindow, modelId },
              type: "setContextWindow",
            })
          }
          onSelectSpeed={(modelId, speed) =>
            send(vscodeApi, { data: { modelId, speed }, type: "setSpeed" })
          }
          onSelectThinkingLevel={(modelId, level) =>
            send(vscodeApi, {
              data: { level, modelId },
              type: "setThinkingLevel",
            })
          }
          sessionContextWindow={sessionModelDetails?.selectedContextWindow}
          sessionModel={state.sessionModel}
          sessionThinkingLevel={sessionModelDetails?.selectedReasoningLevel}
        />
      ) : null}
      <main
        aria-label="Plan preview"
        className="tc-plan-preview__content"
        data-testid="plan-content"
        ref={contentRef}
        tabIndex={-1}
      >
        <MarkdownBody
          markdown={state.bodyMarkdown}
          onOpenFile={openPlanFile}
          onOpenLink={openPlanLink}
          resolvePaths={resolvePaths}
          sourceLineMap={state.bodyLineMap}
        />
        <h2
          className="tc-plan-preview__todos-count"
          data-testid="plan-todos-count"
          id="plan-todos-heading"
        >
          {todoCountLabel(state.todos.length)}
        </h2>
        <hr className="tc-plan-preview__divider" />
        <TodoList labelledBy="plan-todos-heading" todos={state.todos} />
      </main>
      {find.open ? <PlanFindWidget {...find.widgetProps} /> : null}
      <PlanSelectionActionButton onAdd={sendSelection} />
    </div>
  );
}
