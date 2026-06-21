import type {
  AskQuestionResult,
  AskQuestionWireRequest,
  ControlRequestFrame,
} from "../../serveClient/protocol";
import type { ServeAttachment, ServeEvent } from "../../serveClient/wire";
import type { ParticipantPlanState } from "../participant/planState";

export type TomcatUiMode = "both" | "participant" | "webview";
export type FrontendOwnerKind = "participant" | "webview";

export interface WebviewMessageBlock {
  id: string;
  kind: "assistant" | "error" | "notice" | "user";
  text: string;
  type: "message";
}

export interface WebviewThinkingBlock {
  id: string;
  text: string;
  type: "thinking";
}

export interface WebviewToolDisplayFile {
  file: string;
  kind: "file";
}

export interface WebviewToolDisplayPlan {
  kind: "plan";
  plan: string;
}

export interface WebviewToolDisplayText {
  kind: "text";
  text: string;
}

export type WebviewToolDisplay =
  | WebviewToolDisplayFile
  | WebviewToolDisplayPlan
  | WebviewToolDisplayText;

export interface WebviewToolCard {
  display?: WebviewToolDisplay;
  id: string;
  isError: boolean;
  status: "complete" | "running" | "streaming";
  summary?: string;
  toolCallId: string;
  toolName: string;
  type: "tool";
}

export interface WebviewPlanFileRef {
  path: string;
  planId?: string | null;
  state: ParticipantPlanState | null;
}

export interface WebviewPlanFileCard extends WebviewPlanFileRef {
  id: string;
  type: "plan";
}

export interface WebviewApprovalCard {
  id: string;
  request: AskQuestionWireRequest;
  resolved: boolean;
  sessionId?: string | null;
  type: "approval";
}

export interface WebviewPendingAttachment {
  attachment: ServeAttachment;
  id: string;
  kind: ServeAttachment["kind"];
  label: string;
  mimeType?: string | null;
  path?: string | null;
}

export interface WebviewSessionSnapshot {
  busy: boolean;
  conflictMessage?: string | null;
  contextRatio?: number | null;
  model?: string | null;
  ownedByThisFrontend: boolean;
  owner: FrontendOwnerKind | null;
  pendingAttachments: WebviewPendingAttachment[];
  planFile?: WebviewPlanFileRef | null;
  planId?: string | null;
  planState?: ParticipantPlanState | null;
  sessionId: string;
  timeline: WebviewTimelineItem[];
}

export interface WebviewSessionTab {
  busy: boolean;
  isCurrent: boolean;
  ownedByThisFrontend: boolean;
  owner: FrontendOwnerKind | null;
  sessionId: string;
  updatedAt: number | null;
}

export interface WebviewStateSnapshot {
  activeSessionId: string | null;
  availableModels: string[];
  ready: boolean;
  sessionViews: Record<string, WebviewSessionSnapshot>;
  sessions: WebviewSessionTab[];
  uiMode: TomcatUiMode;
}

export type WebviewTimelineItem =
  | WebviewApprovalCard
  | WebviewMessageBlock
  | WebviewPlanFileCard
  | WebviewThinkingBlock
  | WebviewToolCard;

export type HostEventFrameContent =
  | ControlRequestFrame
  | ServeEvent
  | {
      type: "__test.capture_dom";
    };

export type HostToWebviewFrame =
  | {
      channel: "event";
      content: HostEventFrameContent;
      done?: boolean;
      messageId: string;
    }
  | {
      channel: "state";
      content: WebviewStateSnapshot;
      messageId: string;
    };

export type WebviewIntent =
  | {
      messageId: string;
      type: "answerQuestion";
      data: {
        requestId: string;
        result: AskQuestionResult;
      };
    }
  | {
      messageId: string;
      type: "applyEdit";
      data: {
        toolCallId: string;
      };
    }
  | {
      messageId: string;
      type: "closeSession";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "interrupt";
      data?: {
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "listSessions";
    }
  | {
      messageId: string;
      type: "newSession";
      data?: {
        cwd?: string | null;
      };
    }
  | {
      messageId: string;
      type: "openDiff";
      data: {
        toolCallId: string;
      };
    }
  | {
      messageId: string;
      type: "prompt";
      data: {
        sessionId?: string | null;
        text: string;
      };
    }
  | {
      messageId: string;
      type: "pickAttachment";
      data?: {
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "ready";
    }
  | {
      messageId: string;
      type: "removeAttachment";
      data: {
        attachmentId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "setModel";
      data: {
        modelId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "setPlanMode";
      data: {
        action: "build" | "enter" | "exit";
        planId?: string | null;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "steer";
      data: {
        sessionId?: string | null;
        text: string;
      };
    }
  | {
      messageId: string;
      type: "openPlanFile";
      data: {
        path: string;
      };
    }
  | {
      messageId: string;
      type: "switchSession";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "__test.dom_snapshot";
      data: {
        activeSessionId: string | null;
        approvalCount: number;
        hasConflict: boolean;
        html: string;
        messageTexts: string[];
        sessionTabs: string[];
        toolTitles: string[];
      };
    };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isAskQuestionResultShape(value: unknown): value is AskQuestionResult {
  return (
    isRecord(value) &&
    Array.isArray(value.answers) &&
    typeof value.cancelled === "boolean"
  );
}

export function isHostToWebviewFrame(value: unknown): value is HostToWebviewFrame {
  return (
    isRecord(value) &&
    isString(value.messageId) &&
    ((value.channel === "state" && isRecord(value.content)) ||
      (value.channel === "event" && isRecord(value.content)))
  );
}

export function isWebviewIntent(value: unknown): value is WebviewIntent {
  if (!isRecord(value) || !isString(value.messageId) || !isString(value.type)) {
    return false;
  }

  switch (value.type) {
    case "ready":
    case "listSessions":
      return true;
    case "pickAttachment":
      return value.data === undefined || isRecord(value.data);
    case "prompt":
    case "steer":
      return isRecord(value.data) && isString(value.data.text);
    case "interrupt":
      return value.data === undefined || isRecord(value.data);
    case "setModel":
      return isRecord(value.data) && isString(value.data.modelId);
    case "setPlanMode":
      return (
        isRecord(value.data) &&
        (value.data.action === "build" ||
          value.data.action === "enter" ||
          value.data.action === "exit")
      );
    case "newSession":
      return value.data === undefined || isRecord(value.data);
    case "switchSession":
    case "closeSession":
      return isRecord(value.data) && isString(value.data.sessionId);
    case "openDiff":
    case "applyEdit":
      return isRecord(value.data) && isString(value.data.toolCallId);
    case "openPlanFile":
      return isRecord(value.data) && isString(value.data.path);
    case "removeAttachment":
      return isRecord(value.data) && isString(value.data.attachmentId);
    case "answerQuestion":
      return (
        isRecord(value.data) &&
        isString(value.data.requestId) &&
        isAskQuestionResultShape(value.data.result)
      );
    case "__test.dom_snapshot":
      return (
        isRecord(value.data) &&
        Array.isArray(value.data.messageTexts) &&
        Array.isArray(value.data.sessionTabs) &&
        Array.isArray(value.data.toolTitles) &&
        typeof value.data.approvalCount === "number" &&
        typeof value.data.hasConflict === "boolean" &&
        typeof value.data.html === "string"
      );
    default:
      return false;
  }
}

export function createHostFrameMessageId(prefix: string): string {
  const random = Math.random().toString(36).slice(2, 10);
  return `${prefix}-${Date.now()}-${random}`;
}

export class PendingMessageTracker<T> {
  private readonly pending = new Map<
    string,
    {
      reject(error: Error): void;
      resolve(value: T): void;
      timeout: NodeJS.Timeout;
    }
  >();

  create(
    messageId: string,
    timeoutMs: number,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(messageId);
        reject(new Error(`Timed out waiting for webview message ${messageId}`));
      }, timeoutMs).unref();
      this.pending.set(messageId, { resolve, reject, timeout });
    });
  }

  resolve(messageId: string, value: T): boolean {
    const pending = this.pending.get(messageId);
    if (!pending) {
      return false;
    }
    clearTimeout(pending.timeout);
    this.pending.delete(messageId);
    pending.resolve(value);
    return true;
  }

  rejectAll(error: Error): void {
    for (const [messageId, pending] of this.pending) {
      clearTimeout(pending.timeout);
      pending.reject(error);
      this.pending.delete(messageId);
    }
  }
}
