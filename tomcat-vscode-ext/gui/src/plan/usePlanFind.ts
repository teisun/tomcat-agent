import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
  type RefObject,
} from "react";

import type { PlanFindWidgetProps } from "./PlanFindWidget";
import {
  isPlanFindDecorationMutationInProgress,
} from "./planFindHighlight";
import { PlanFindController } from "./planFindController";

export interface UsePlanFindOptions {
  contentRef: RefObject<HTMLElement | null>;
  /** A state object/version whose change means the rendered plan copy changed. */
  contentVersion: unknown;
}

export interface UsePlanFindResult {
  open: boolean;
  openFind(): void;
  setFindQuery(query: string): void;
  widgetProps: PlanFindWidgetProps;
}

/**
 * React lifecycle adapter for the framework-independent PlanFindController.
 *
 * The preview supplies only its rendered content root/version; all Find state,
 * highlighting and navigation remain in the controller and are observed through
 * React's standard external-store subscription API.
 */
export function usePlanFind({
  contentRef,
  contentVersion,
}: UsePlanFindOptions): UsePlanFindResult {
  const controllerRef = useRef<PlanFindController | null>(null);
  if (controllerRef.current === null) {
    controllerRef.current = new PlanFindController();
  }
  const controller = controllerRef.current;
  const inputRef = useRef<HTMLInputElement>(null);
  const snapshot = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );

  // This runs after PlanPreviewApp has committed the latest Markdown/todos DOM.
  useLayoutEffect(() => {
    controller.setSearchRoot(contentRef.current);
    controller.setContentVersion(contentVersion);
  }, [contentRef, contentVersion, controller]);

  // Path resolution and Mermaid rendering can replace descendant nodes after
  // React's state commit. Observe that rendered DOM so Find never keeps ranges
  // into detached text nodes or a stale match count.
  useEffect(() => {
    const root = contentRef.current;
    if (!root || typeof MutationObserver !== "function") {
      return;
    }
    const observer = new MutationObserver(() => {
      if (!isPlanFindDecorationMutationInProgress()) {
        controller.notifyRenderedContentChanged();
      }
    });
    observer.observe(root, {
      characterData: true,
      childList: true,
      subtree: true,
    });
    return () => observer.disconnect();
  }, [contentRef, contentVersion, controller]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        controller.open();
        return;
      }
      if (event.key === "Escape" && controller.getSnapshot().open) {
        event.preventDefault();
        controller.close();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [controller]);

  useEffect(
    () => () => {
      controller.dispose();
    },
    [controller],
  );

  const openFind = useCallback(() => controller.open(), [controller]);
  const setFindQuery = useCallback(
    (query: string) => controller.setQuery(query),
    [controller],
  );

  return {
    open: snapshot.open,
    openFind,
    setFindQuery,
    widgetProps: {
      activeIndex: snapshot.activeIndex,
      inputRef,
      onClose: controller.close,
      onNext: controller.moveNext,
      onPrevious: controller.movePrev,
      onQueryChange: controller.setQuery,
      query: snapshot.query,
      total: snapshot.matches.length,
    },
  };
}
