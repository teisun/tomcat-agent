export type TomcatUiMode = "both" | "participant" | "webview";

export interface WebviewDomAction {
  kind: "clickTestId" | "scrollToEdge" | "setInputValue" | "setRootWidth";
  edge?: "bottom" | "top";
  index?: number;
  testId?: string;
  value?: string;
  widthPx?: number | null;
}
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

export type WebviewToolStatus = "complete" | "running" | "streaming";

export interface WebviewToolCard {
  display?: WebviewToolDisplay;
  id: string;
  isError: boolean;
  status: WebviewToolStatus;
  summary?: string;
  toolCallId: string;
  toolName: string;
  type: "tool";
}

export type WebviewPlanState =
  | "chat"
  | "planning"
  | "executing"
  | "pending"
  | "completed";

export interface WebviewPlanFileRef {
  path: string;
  planId?: string | null;
  state: WebviewPlanState | null;
}

export interface WebviewPlanFileCard extends WebviewPlanFileRef {
  id: string;
  type: "plan";
}

export interface WebviewApprovalOption {
  id: string;
  label: string;
  recommended?: boolean;
}

export interface WebviewApprovalQuestion {
  id: string;
  options: WebviewApprovalOption[];
  prompt: string;
}

export const CUSTOM_OPTION_ID = "__custom__";

export interface AskQuestionAnswer {
  customText?: string | null;
  optionIds: string[];
  pickedRecommended: boolean;
  questionId: string;
  skipped?: boolean;
}

export interface AskQuestionResult {
  answers: AskQuestionAnswer[];
  cancelled: boolean;
}

export interface WebviewApprovalCard {
  id: string;
  request: {
    questions: WebviewApprovalQuestion[];
    requestId: string;
    responseEvent: string;
  };
  resolved: boolean;
  sessionId?: string | null;
  type: "approval";
}

export interface WebviewPendingAttachment {
  attachment: {
    dataBase64?: string | null;
    fileId?: string | null;
    kind: "file" | "image";
    mimeType?: string | null;
  };
  id: string;
  kind: "file" | "image";
  label: string;
  mimeType?: string | null;
  path?: string | null;
}

export interface WebviewSessionSnapshot {
  busy: boolean;
  conflictMessage?: string | null;
  contextRatio?: number | null;
  model?: string | null;
  thinkingLevel?: string | null;
  ownedByThisFrontend: boolean;
  owner: FrontendOwnerKind | null;
  pendingAttachments: WebviewPendingAttachment[];
  planFile?: WebviewPlanFileRef | null;
  planId?: string | null;
  planState?: WebviewPlanState | null;
  sessionId: string;
  timeline: WebviewTimelineItem[];
}

export interface WebviewSessionTab {
  busy: boolean;
  isCurrent: boolean;
  ownedByThisFrontend: boolean;
  owner: FrontendOwnerKind | null;
  sessionId: string;
  title: string | null;
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

export type HostToWebviewFrame =
  | {
      channel: "event";
      content:
        | {
            type: "__test.capture_dom";
          }
        | {
            action: WebviewDomAction;
            type: "__test.dom_action";
          }
        | Record<string, unknown>;
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
      type: "applyEdit" | "openDiff";
      data: {
        toolCallId: string;
      };
    }
  | {
      messageId: string;
      type: "closeSession" | "switchSession";
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
      type: "listSessions" | "ready";
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
      type: "prompt" | "steer";
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
      type: "setThinkingLevel";
      data: {
        level: "high" | "low" | "medium" | "xhigh";
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
      type: "openPlanFile";
      data: {
        path: string;
      };
    }
  | {
      messageId: string;
      type: "__test.dom_snapshot";
      data: {
        activeSessionId: string | null;
        approvalCount: number;
        approvalInputTestIds: string[];
        approvalOptionStates: Array<{
          selected: boolean;
          testId: string;
        }>;
        composerControlMetrics: Record<
          string,
          {
            top: number;
            width: number;
          }
        >;
        composerRowCount: number;
        disabledTestIds: string[];
        expandedThinkingCount: number;
        expandedToolTitles: string[];
        hasConflict: boolean;
        html: string;
        jumpToLatestVisible: boolean;
        latestUserTopWithinStream: number | null;
        messageTexts: string[];
        overflowAnchor: string | null;
        sessionTabs: string[];
        sessionGroupHeaders: string[];
        sessionMoreButtons: string[];
        stickyPromptText: string | null;
        streamMetrics: {
          clientHeight: number;
          distanceFromBottom: number;
          scrollHeight: number;
          scrollTop: number;
        };
        timelineKinds: string[];
        toolBodyMetrics: Array<{
          clientHeight: number;
          expanded: boolean;
          scrollHeight: number;
          title: string;
        }>;
        toolTitles: string[];
      };
    };

export interface VsCodeApiLike {
  postMessage(message: WebviewIntent): void;
  setState?(state: unknown): void;
}
