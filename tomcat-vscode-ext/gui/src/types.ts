export type TomcatUiMode = "both" | "participant" | "webview";
export type FrontendOwnerKind = "participant" | "webview";

export interface WebviewMessageBlock {
  id: string;
  kind: "assistant" | "error" | "notice" | "thinking" | "user";
  text: string;
}

export interface WebviewToolCard {
  display?: {
    file?: string;
    kind: "file" | "plan" | "text";
    plan?: string;
    text?: string;
  };
  isError: boolean;
  status: "complete" | "running" | "streaming";
  summary?: string;
  toolCallId: string;
  toolName: string;
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

export interface WebviewApprovalCard {
  request: {
    questions: WebviewApprovalQuestion[];
    requestId: string;
    responseEvent: string;
  };
  resolved: boolean;
  sessionId?: string | null;
}

export interface WebviewSessionSnapshot {
  approvals: WebviewApprovalCard[];
  busy: boolean;
  conflictMessage?: string | null;
  messages: WebviewMessageBlock[];
  model?: string | null;
  ownedByThisFrontend: boolean;
  owner: FrontendOwnerKind | null;
  planId?: string | null;
  planState?: string | null;
  sessionId: string;
  tools: WebviewToolCard[];
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

export type HostToWebviewFrame =
  | {
      channel: "event";
      content:
        | {
            type: "__test.capture_dom";
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
        result: {
          answers: Array<{
            customText?: string | null;
            optionIds: string[];
            pickedRecommended: boolean;
            questionId: string;
            skipped?: boolean;
          }>;
          cancelled: boolean;
        };
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

export interface VsCodeApiLike {
  postMessage(message: WebviewIntent): void;
  setState?(state: unknown): void;
}
