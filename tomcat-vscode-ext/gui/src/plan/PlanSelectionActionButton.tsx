import { useEffect, useRef, useState } from "react";

interface FloatingPosition {
  left: number;
  top: number;
}

const PLAN_CONTENT_SELECTOR = '[data-testid="plan-content"]';

/** Read the current selection text if it lives inside the plan content. */
function readContainedSelection(): { rect: DOMRect; text: string } | null {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) {
    return null;
  }
  const text = selection.toString();
  if (!text.trim()) {
    return null;
  }
  const content = document.querySelector(PLAN_CONTENT_SELECTOR);
  const anchorNode = selection.anchorNode;
  if (!content || !anchorNode || !content.contains(anchorNode)) {
    return null;
  }
  const rect = selection.getRangeAt(0).getBoundingClientRect();
  if (rect.width === 0 && rect.height === 0) {
    return null;
  }
  return { rect, text };
}

/**
 * Cursor/Medium-style floating action shown near a non-empty text selection in
 * the plan preview. Clicking it forwards the selected text to the Tomcat chat.
 * The button hides on scroll, blur, or when the selection collapses.
 */
export function PlanSelectionActionButton({
  onAdd,
}: {
  onAdd(text: string): void;
}) {
  const [position, setPosition] = useState<FloatingPosition | null>(null);
  const textRef = useRef("");

  useEffect(() => {
    let frame = 0;
    const recompute = () => {
      const found = readContainedSelection();
      if (!found) {
        textRef.current = "";
        setPosition(null);
        return;
      }
      textRef.current = found.text;
      setPosition({
        left: found.rect.left + found.rect.width / 2,
        top: Math.max(found.rect.top - 6, 24),
      });
    };
    const scheduleRecompute = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(recompute);
    };
    const hide = () => {
      textRef.current = "";
      setPosition(null);
    };
    document.addEventListener("selectionchange", scheduleRecompute);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("blur", hide);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("selectionchange", scheduleRecompute);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("blur", hide);
    };
  }, []);

  if (!position) {
    return null;
  }

  return (
    <button
      className="tc-plan-selection-action"
      data-testid="plan-selection-add"
      // Keep the selection alive so the click reads the same text.
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => {
        // Prefer the live selection (same source the right-click command reads)
        // and fall back to the last captured text if it has already collapsed.
        const text = readContainedSelection()?.text ?? textRef.current;
        setPosition(null);
        if (text.trim()) {
          onAdd(text);
        }
      }}
      style={{ left: position.left, top: position.top }}
      type="button"
    >
      Add to Tomcat Chat
    </button>
  );
}
