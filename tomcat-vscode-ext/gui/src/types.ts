export type TomcatUiMode = "both" | "participant" | "webview";
export type WebviewReferenceKind = "selection" | "file";

export type WebviewMessageSegment =
  | {
      text: string;
      type: "text";
    }
  | {
      kind: WebviewReferenceKind;
      label: string;
      lineEnd?: number | null;
      lineStart?: number | null;
      path: string;
      text?: string | null;
      type: "reference";
    };

export type WebviewReference = Extract<WebviewMessageSegment, { type: "reference" }>;

export interface ContextSearchMatch {
  description?: string | null;
  reference: WebviewReference;
}

export interface WebviewDomAction {
  kind:
    | "clickTestId"
    | "dragOverTestId"
    | "dragLeaveTestId"
    | "scrollIntoView"
    | "scrollToEdge"
    | "setInputValue"
    | "setRootWidth";
  edge?: "bottom" | "top";
  index?: number;
  scrollBlock?: "center" | "end" | "nearest" | "start";
  testId?: string;
  value?: string;
  widthPx?: number | null;
}
export type FrontendOwnerKind = "participant" | "webview";

export interface WebviewMessageBlock {
  assistantMessageId?: string;
  deliveryError?: string | null;
  deliveryState?: "failed" | "pending";
  id: string;
  kind: "assistant" | "error" | "notice" | "user" | "warn";
  retryable?: boolean;
  segments?: WebviewMessageSegment[];
  submitKind?: "prompt" | "steer";
  text: string;
  type: "message";
}

export interface WebviewThinkingBlock {
  assistantMessageId?: string;
  id: string;
  summaryTitle?: string | null;
  text: string;
  type: "thinking";
}

export interface WebviewBoundaryBlock {
  coveredCount?: number | null;
  id: string;
  summary?: string | null;
  type: "boundary";
}

export interface WebviewCheckpoint {
  changedFiles: string[];
  createdAt: string;
  id: string;
  kind: string;
  label?: string | null;
  messageAnchor?: string | null;
}

export interface WebviewCheckpointMarker {
  changedFiles: string[];
  checkpointId: string;
  createdAt: string;
  id: string;
  kind: string;
  label?: string | null;
  messageAnchor: string;
  type: "checkpoint";
}

export interface WebviewTodo {
  content: string;
  id: string;
  status: "cancelled" | "completed" | "in_progress" | "pending";
}

export type FileDiffTag = "add" | "ctx" | "del";

export interface FileDiffLine {
  newLine?: number | null;
  oldLine?: number | null;
  tag: FileDiffTag;
  text: string;
}

export interface WebviewToolDisplayFile {
  added?: number | null;
  diff?: FileDiffLine[] | null;
  file: string;
  kind: "file";
  removed?: number | null;
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

export type WebviewToolStatus = "complete" | "interrupted" | "running" | "streaming";

export interface WebviewToolDiffStat {
  added: number;
  removed: number;
}

export interface WebviewToolCard {
  args?: Record<string, unknown>;
  assistantMessageId?: string;
  display?: WebviewToolDisplay;
  diff?: FileDiffLine[];
  diffStat?: WebviewToolDiffStat;
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
  overview?: string;
  title?: string;
  todos?: WebviewTodo[];
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
    filename?: string | null;
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
  checkpoints?: WebviewCheckpoint[];
  conflictMessage?: string | null;
  contextRatio?: number | null;
  hasMoreHistory?: boolean;
  historyLoading?: boolean;
  model?: string | null;
  planTodos: WebviewTodo[];
  sessionTodos: WebviewTodo[];
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
  availableModelCapabilities?: Record<string, string[]>;
  availableModels: string[];
  buildModel?: string;
  modelAdminSupported: boolean;
  ready: boolean;
  sessionViews: Record<string, WebviewSessionSnapshot>;
  sessions: WebviewSessionTab[];
  uiMode: TomcatUiMode;
}

export type WebviewTimelineItem =
  | WebviewApprovalCard
  | WebviewBoundaryBlock
  | WebviewCheckpointMarker
  | WebviewMessageBlock
  | WebviewPlanFileCard
  | WebviewThinkingBlock
  | WebviewToolCard;

export type HostToWebviewFrame =
  | {
      channel: "event";
      content:
        | {
            matches: ContextSearchMatch[];
            query: string;
            requestId: string;
            sessionId?: string | null;
            truncated: boolean;
            type: "contextSearchResult";
            workspaceAvailable?: boolean;
          }
        | {
            reference: WebviewReference;
            sessionId?: string | null;
            type: "insertReference";
          }
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
      type: "loadOlderHistory";
      data: {
        sessionId: string;
      };
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
        segments?: WebviewMessageSegment[];
        sessionId?: string | null;
        text: string;
        userMessageId?: string;
      };
    }
  | {
      messageId: string;
      type: "pickContext";
      data?: {
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "retryUserMessage";
      data: {
        messageId: string;
        sessionId: string;
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
      type: "setBuildModel";
      data: {
        modelId: string;
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
      type: "openModelSettings";
      data?: {
        route?: "models" | null;
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
      type: "openFile" | "openPlanFile";
      data: {
        path: string;
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
      type: "resolveDrop";
      data: {
        sessionId?: string | null;
        uris: string[];
      };
    }
  | {
      messageId: string;
      type: "searchContext";
      data: {
        kind?: "file";
        query: string;
        requestId: string;
        sessionId?: string | null;
      };
    }
  | {
      messageId: string;
      type: "showWarningMessage";
      data: {
        message: string;
      };
    }
  | {
      messageId: string;
      type: "listCheckpoints";
      data: {
        sessionId: string;
      };
    }
  | {
      messageId: string;
      type: "restoreCheckpoint";
      data: {
        checkpointId: string;
        revertFiles: boolean;
        sessionId: string;
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
        composerFooterPlanStatus: string | null;
        composerPlanStatusInBarCount: number;
        composerRowCount: number;
        ctxLabel: string | null;
        disabledTestIds: string[];
        expandedThinkingCount: number;
        expandedToolTitles: string[];
        fileChipTopWithinStream: number | null;
        fileChipVisible: boolean;
        hasConflict: boolean;
        historyLoaderVisible: boolean;
        html: string;
        jumpToLatestVisible: boolean;
        latestUserTopWithinStream: number | null;
        messageTexts: string[];
        modelDropdownBottom: number | null;
        modelDropdownFullyVisible: boolean;
        modelDropdownHeight: number;
        modelDropdownLeft: number | null;
        modelDropdownRight: number | null;
        modelDropdownTop: number | null;
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
        assistantResponseGroups: number;
        groupFoldTitles: string[];
        userPromptPill: boolean;
        assistantNoCard: boolean;
        planCardCount: number;
        planFooterSameRow: boolean;
        planCardTodoCountText: string | null;
        planCardTitleText: string | null;
        planNoticeReplayed: boolean;
        planStateText: string | null;
        progressRow: boolean;
        planTodos: number;
        todoWidgetExpanded: boolean;
        todoWidgetItemCount: number;
        todoWidgetTitle: string | null;
        todoWidgetVisible: boolean;
        toolRowFlat: boolean;
        toolRowExpandable: boolean;
        ellipsisAboveGroupHeader: boolean;
        leftGuideLine: boolean;
        toolRowCount: number;
        toolCardCount: number;
        actionToolRowCount: number;
        editDiffBadgeCount: number;
        commandBlockCount: number;
      };
    };

export interface VsCodeApiLike {
  postMessage(message: WebviewIntent): void;
  setState?(state: unknown): void;
}
