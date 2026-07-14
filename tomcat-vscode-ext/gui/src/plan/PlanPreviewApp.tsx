import { useCallback, useEffect, useRef, useState } from "react";

import {
  isPlanPreviewHostFrame,
  type PlanEditorMode,
  type PlanPreviewDomAction,
  type PlanPreviewDomSnapshot,
  type PlanPreviewIntent,
  type PlanPreviewStateSnapshot,
  type PlanToolbarStyle,
  type VsCodeApiLike,
} from "../../../src/shared/planPreviewProtocol";
import { MarkdownBody } from "../components/MarkdownBody";
import { PlanActionStrip } from "../components/PlanActionStrip";
import { TodoList } from "../components/TodoList";
import { PlanSelectionActionButton } from "./PlanSelectionActionButton";

type DistributiveOmit<T, K extends keyof T> = T extends unknown ? Omit<T, K> : never;
type PlanIntentWithoutId = DistributiveOmit<PlanPreviewIntent, "messageId">;

function send(vscodeApi: VsCodeApiLike<PlanPreviewIntent>, message: PlanIntentWithoutId): void {
  vscodeApi.postMessage({
    ...message,
    messageId: `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  } as PlanPreviewIntent);
}

function todoCountLabel(count: number): string {
  return `${count} ${count === 1 ? "To-do" : "To-dos"}`;
}

/**
 * Best-effort 1-based line range of a selection inside the plan source. The
 * preview shows rendered markdown, so we locate the first and last non-empty
 * selected lines back in the raw source by substring match. Returns null when
 * they cannot be found (the caller then omits line numbers from the reference).
 */
function deriveSelectionLines(
  raw: string,
  selectedText: string,
): { lineEnd: number; lineStart: number } | null {
  const selLines = selectedText
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  if (selLines.length === 0) {
    return null;
  }
  const rawLines = raw.split("\n");
  const firstNeedle = selLines[0];
  const lastNeedle = selLines[selLines.length - 1];
  const startIdx = rawLines.findIndex((line) => line.includes(firstNeedle));
  if (startIdx === -1) {
    return null;
  }
  let endIdx = startIdx;
  for (let i = rawLines.length - 1; i >= startIdx; i -= 1) {
    if (rawLines[i].includes(lastNeedle)) {
      endIdx = i;
      break;
    }
  }
  return { lineEnd: endIdx + 1, lineStart: startIdx + 1 };
}

/** Read the rendered DOM for E2E assertions (test-only). */
function readDomSnapshot(state: PlanPreviewStateSnapshot | null): PlanPreviewDomSnapshot {
  const strip = document.querySelector('[data-testid="plan-action-strip"]');
  const select = document.querySelector<HTMLSelectElement>(
    '[data-testid="plan-build-model-select"]',
  );
  const countEl = document.querySelector('[data-testid="plan-todos-count"]');
  const items = document.querySelectorAll('[data-testid="plan-todo-item"]');
  const icons = document.querySelectorAll(".tc-plan-todo__icon");
  const body = document.querySelector('[data-testid="plan-markdown-body"]');
  const source = document.querySelector('[data-testid="plan-source"]');
  const options = select
    ? Array.from(select.options)
        .map((option) => option.value)
        .filter((value) => value !== "")
    : [];
  const todoIconSizes = Array.from(icons).map((icon) =>
    Math.round(icon.getBoundingClientRect().width),
  );
  const mode: PlanEditorMode = source ? "markdown" : "preview";
  const toolbarStyle: PlanToolbarStyle = state?.toolbarStyle ?? "native";
  const mermaidSvgCount = document.querySelectorAll(
    '[data-testid="plan-mermaid"] svg',
  ).length;
  return {
    bodyHasContent: Boolean(body && (body.textContent ?? "").trim().length > 0),
    buildModelOptions: options,
    buildModelValue: select ? select.value : "",
    hasActionStrip: Boolean(strip),
    markdownSourceText: source ? source.textContent ?? "" : null,
    mermaidSvgCount,
    mode,
    selectionButtonVisible: Boolean(
      document.querySelector('[data-testid="plan-selection-add"]'),
    ),
    todoCountText: countEl ? countEl.textContent : null,
    todoIconSizes,
    todoItemCount: items.length,
    toolbarStyle,
  };
}

function runDomAction(action: PlanPreviewDomAction): void {
  switch (action.kind) {
    case "clickBuild":
      document.querySelector<HTMLButtonElement>('[data-testid="plan-build"]')?.click();
      return;
    case "clickSelectionAdd":
      document.querySelector<HTMLButtonElement>('[data-testid="plan-selection-add"]')?.click();
      return;
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
      const select = document.querySelector<HTMLSelectElement>(
        '[data-testid="plan-build-model-select"]',
      );
      if (select) {
        const setter = Object.getOwnPropertyDescriptor(
          window.HTMLSelectElement.prototype,
          "value",
        )?.set;
        setter?.call(select, action.modelId);
        select.dispatchEvent(new Event("change", { bubbles: true }));
      }
      return;
    }
  }
}

export function PlanPreviewApp({
  vscodeApi,
}: {
  vscodeApi: VsCodeApiLike<PlanPreviewIntent>;
}) {
  const [state, setState] = useState<PlanPreviewStateSnapshot | null>(null);
  const stateRef = useRef<PlanPreviewStateSnapshot | null>(state);
  stateRef.current = state;

  const sendSelection = useCallback(
    (selectedText: string) => {
      const trimmed = selectedText.trim();
      if (!trimmed) {
        return;
      }
      const lines = deriveSelectionLines(stateRef.current?.raw ?? "", trimmed);
      send(vscodeApi, {
        data: lines
          ? { lineEnd: lines.lineEnd, lineStart: lines.lineStart, text: trimmed }
          : { text: trimmed },
        type: "addSelectionToChat",
      });
    },
    [vscodeApi],
  );

  useEffect(() => {
    const handleMessage = (event: MessageEvent<unknown>) => {
      const frame = event.data;
      if (!isPlanPreviewHostFrame(frame)) {
        return;
      }
      if (frame.channel === "state") {
        setState(frame.content);
        return;
      }
      if (frame.content.type === "captureSelectionForChat") {
        const selection = window.getSelection();
        sendSelection(selection ? selection.toString() : "");
        return;
      }
      if (frame.content.type === "__test.capture_dom") {
        vscodeApi.postMessage({
          data: readDomSnapshot(stateRef.current),
          messageId: frame.messageId,
          type: "__test.dom_snapshot",
        } as unknown as PlanPreviewIntent);
        return;
      }
      if (frame.content.type === "__test.dom_action") {
        runDomAction(frame.content.action);
      }
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, { type: "plan.ready" });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [sendSelection, vscodeApi]);

  if (!state) {
    return (
      <div className="tc-plan-preview tc-plan-preview--loading" data-testid="plan-loading">
        Loading plan…
      </div>
    );
  }

  const isHybrid = state.toolbarStyle === "hybrid";

  return (
    <div className="tc-plan-preview">
      <div className="tc-plan-preview__content" data-testid="plan-content">
        {isHybrid ? (
          <PlanActionStrip
            availableModels={state.availableModels}
            buildModel={state.buildModel}
            canBuild={state.canBuild}
            onBuild={() => send(vscodeApi, { type: "build" })}
            onSetBuildModel={(modelId) =>
              send(vscodeApi, { data: { modelId }, type: "setBuildModel" })
            }
          />
        ) : null}
        {state.mode === "preview" ? (
          <>
            <MarkdownBody
              markdown={state.bodyMarkdown}
              onOpenLink={(href) => send(vscodeApi, { data: { href }, type: "openLink" })}
            />
            <div className="tc-plan-preview__todos-count" data-testid="plan-todos-count">
              {todoCountLabel(state.todos.length)}
            </div>
            <hr className="tc-plan-preview__divider" />
            <TodoList todos={state.todos} />
          </>
        ) : (
          <div className="tc-plan-preview__markdown" data-testid="plan-source-wrapper">
            <div className="tc-plan-preview__markdown-actions">
              <button
                className="tc-button tc-button--secondary"
                data-testid="plan-open-editor"
                onClick={() => send(vscodeApi, { type: "openInTextEditor" })}
                type="button"
              >
                Open in Editor
              </button>
            </div>
            <pre className="tc-plan-preview__source" data-testid="plan-source">
              <code>{state.raw}</code>
            </pre>
          </div>
        )}
      </div>
      <PlanSelectionActionButton onAdd={sendSelection} />
    </div>
  );
}
