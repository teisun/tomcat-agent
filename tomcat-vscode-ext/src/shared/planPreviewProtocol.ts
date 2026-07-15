import { acquireVsCodeApiLike, type VsCodeApiLike } from "./settingsProtocol";

export { acquireVsCodeApiLike };
export type { VsCodeApiLike };

export type PlanTodoStatus = "cancelled" | "completed" | "in_progress" | "pending";

export interface PlanTodo {
  content: string;
  id: string;
  status: PlanTodoStatus;
}

export type PlanFileState = "completed" | "executing" | "pending" | "planning";

/**
 * Which surface hosts the plan's action controls (Build / model). `native` puts
 * them on the VS Code editor title bar as commands; `hybrid` keeps a slim in-body
 * action strip. Temporary A/B switch driven by `tomcat.plan.toolbarStyle`.
 */
export type PlanToolbarStyle = "hybrid" | "native";

/**
 * Everything the plan preview webview needs to render, derived on the host from
 * the `.plan.md` document text plus VS Code config / serve catalog. The custom
 * editor always renders the preview; "Markdown" opens the native text editor
 * instead (a separate editor), so there is no in-webview mode any more.
 */
export interface PlanPreviewStateSnapshot {
  availableModels: string[];
  /** 1-based source file line for each line of `bodyMarkdown` (see planDocument). */
  bodyLineMap: number[];
  bodyMarkdown: string;
  buildModel: string;
  canBuild: boolean;
  overview: string | null;
  path: string;
  planId: string | null;
  raw: string;
  state: PlanFileState | null;
  title: string | null;
  todos: PlanTodo[];
  toolbarStyle: PlanToolbarStyle;
}

/** DOM/state readout the plan preview webview reports back during E2E tests. */
export interface PlanPreviewDomSnapshot {
  bodyHasContent: boolean;
  /** Left inset (px) of the rendered markdown body; proves the content column has
   * deliberate side padding instead of sitting flush against the editor edge. */
  bodyInsetLeft: number | null;
  buildModelOptions: string[];
  buildModelValue: string;
  hasActionStrip: boolean;
  /** Rendered mermaid diagrams (fenced ```mermaid``` blocks turned into SVG). */
  mermaidSvgCount: number;
  selectionButtonVisible: boolean;
  /** Left inset (px) of the action strip's bounding box; ~0 means it spans the
   * full editor width with no leftover VS Code body padding. */
  stripInsetLeft: number | null;
  /** True when the action strip is a sibling of (not nested in) the scroll column. */
  stripOutsideContent: boolean;
  todoCountText: string | null;
  todoIconSizes: number[];
  todoItemCount: number;
  toolbarStyle: PlanToolbarStyle;
}

/** Test-only DOM drive actions issued by the host during E2E tests. */
export type PlanPreviewDomAction =
  | { kind: "clickBuild" }
  | { kind: "clickSelectionAdd" }
  | { kind: "selectBuildModel"; modelId: string }
  | { kind: "selectText"; selector: string };

export type PlanPreviewTestEvent =
  | { type: "__test.capture_dom" }
  | { action: PlanPreviewDomAction; type: "__test.dom_action" };

/**
 * Host → webview events. `captureSelectionForChat` is fired by the right-click
 * command so the webview reads its own live selection and replies with an
 * `addSelectionToChat` intent (the host cannot see a webview's DOM selection).
 */
export type PlanPreviewEvent =
  | { type: "captureSelectionForChat" }
  | PlanPreviewTestEvent;

export type PlanPreviewHostFrame =
  | {
      channel: "event";
      content: PlanPreviewEvent;
      messageId: string;
    }
  | {
      channel: "state";
      content: PlanPreviewStateSnapshot;
      messageId: string;
    };

/** Webview → host reply carrying a captured DOM snapshot (test-only). */
export interface PlanPreviewDomSnapshotReply {
  data: PlanPreviewDomSnapshot;
  messageId: string;
  type: "__test.dom_snapshot";
}

export type PlanPreviewIntent =
  | {
      messageId: string;
      type: "plan.ready";
    }
  | {
      messageId: string;
      type: "openLink";
      data: {
        href: string;
      };
    }
  | {
      messageId: string;
      type: "setBuildModel";
      data: {
        modelId: string;
      };
    }
  | {
      messageId: string;
      type: "build";
    }
  | {
      messageId: string;
      type: "addSelectionToChat";
      data: {
        text: string;
        lineStart?: number;
        lineEnd?: number;
      };
    };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isPlanPreviewIntent(value: unknown): value is PlanPreviewIntent {
  if (!isRecord(value) || typeof value.messageId !== "string" || typeof value.type !== "string") {
    return false;
  }
  switch (value.type) {
    case "plan.ready":
    case "build":
      return true;
    case "openLink":
      return isRecord(value.data) && typeof value.data.href === "string";
    case "setBuildModel":
      return isRecord(value.data) && typeof value.data.modelId === "string";
    case "addSelectionToChat":
      return isRecord(value.data) && typeof value.data.text === "string";
    default:
      return false;
  }
}

export function isPlanPreviewHostFrame(value: unknown): value is PlanPreviewHostFrame {
  if (!isRecord(value) || !isRecord(value.content)) {
    return false;
  }
  return value.channel === "state" || value.channel === "event";
}

export function isPlanPreviewDomSnapshotReply(
  value: unknown,
): value is PlanPreviewDomSnapshotReply {
  return (
    isRecord(value) &&
    value.type === "__test.dom_snapshot" &&
    typeof value.messageId === "string" &&
    isRecord(value.data)
  );
}
