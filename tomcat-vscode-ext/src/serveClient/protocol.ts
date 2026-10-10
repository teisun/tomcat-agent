import { isUiPreferences, t, type UiPreferences } from "../shared/i18n";
import type {
  ControlFrame,
  OutFrame,
  ResponseFrame,
  ServeCommand,
  ServeEvent,
  SharedSlashCommand,
} from "./wire";

export type ControlRequestFrame = Extract<ControlFrame, { type: "control_request" }>;
export type ControlResponseFrame = Extract<ControlFrame, { type: "control_response" }>;
export type ControlCancelFrame = Extract<ControlFrame, { type: "control_cancel" }>;
export type ControlResultFrame = ControlResponseFrame | ControlCancelFrame;
export type RequestCommand = Exclude<
  ServeCommand,
  { type: "control_request" | "control_response" | "control_cancel" }
>;

export interface InitializePayload {
  uiPreferences?: UiPreferences | null;
  protocolVersion: number;
  /** Absolute path of the attachment directory, absent on older servers. */
  attachmentRoot?: string | null;
  capabilities: string[];
  slashCommands?: SharedSlashCommand[];
  sessionId?: string | null;
  serverVersion?: string | null;
}

export interface AskQuestionOption {
  id: string;
  label: string;
  recommended?: boolean;
}

export interface AskQuestion {
  allowCustom?: boolean;
  id: string;
  prompt: string;
  options: AskQuestionOption[];
}

export interface AskQuestionWireRequest {
  requestId: string;
  responseEvent: string;
  sessionId?: string;
  toolCallId?: string;
  questions: AskQuestion[];
}

export interface AskQuestionAnswer {
  questionId: string;
  optionIds: string[];
  customText?: string | null;
  skipped?: boolean;
  pickedRecommended: boolean;
}

export type AskQuestionOutcome =
  | "answered"
  | "skipped"
  | "interrupted"
  | "host_disconnected"
  | "cancelled_unknown";

export interface AskQuestionResult {
  answers: AskQuestionAnswer[];
  /** Legacy compatibility. New peers also send `outcome`; there is no timeout outcome. */
  cancelled: boolean;
  outcome?: AskQuestionOutcome;
}

export function askQuestionOutcome(result: AskQuestionResult): AskQuestionOutcome {
  return result.outcome ?? (result.cancelled ? "cancelled_unknown" : "answered");
}

export interface AskQuestionWireResponse {
  requestId: string;
  result: AskQuestionResult;
}

export interface DisposableLike {
  dispose(): void;
}

export const REQUIRED_SERVE_CAPABILITIES = ["prompt", "ask_question"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

export function isResponseFrame(value: unknown): value is ResponseFrame {
  return isRecord(value) && value.type === "response" && typeof value.success === "boolean";
}

export function isControlFrame(value: unknown): value is ControlFrame {
  return (
    isRecord(value) &&
    typeof value.requestId === "string" &&
    (value.type === "control_request" ||
      value.type === "control_response" ||
      value.type === "control_cancel")
  );
}

export function isWireEvent(value: unknown): value is ServeEvent {
  return isRecord(value) && typeof value.type === "string" && !isResponseFrame(value) && !isControlFrame(value);
}

export function isOutFrame(value: unknown): value is OutFrame {
  return isResponseFrame(value) || isControlFrame(value) || isWireEvent(value);
}

export function parseInitializePayload(payload: unknown): InitializePayload {
  if (!isRecord(payload)) {
    throw new Error("initialize payload must be an object");
  }

  if (typeof payload.protocolVersion !== "number") {
    throw new Error("initialize payload is missing protocolVersion");
  }

  if (!isStringArray(payload.capabilities)) {
    throw new Error("initialize payload is missing capabilities");
  }

  const slashCommands = payload.slashCommands;
  if (slashCommands !== undefined && (!Array.isArray(slashCommands) || !slashCommands.every((command) =>
    isRecord(command) && typeof command.name === "string" && typeof command.usage === "string" && typeof command.summary === "string"
  ))) {
    throw new Error("initialize payload has invalid slashCommands");
  }

  return {
    protocolVersion: payload.protocolVersion,
    ...(isUiPreferences(payload.uiPreferences) ? { uiPreferences: payload.uiPreferences } : {}),
    attachmentRoot:
      typeof payload.attachmentRoot === "string" && payload.attachmentRoot.length > 0
        ? payload.attachmentRoot
        : null,
    capabilities: payload.capabilities,
    slashCommands: (slashCommands ?? []) as SharedSlashCommand[],
    sessionId:
      payload.sessionId === undefined || payload.sessionId === null
        ? null
        : String(payload.sessionId),
    serverVersion:
      payload.serverVersion === undefined || payload.serverVersion === null
        ? null
        : typeof payload.serverVersion === "string"
          ? payload.serverVersion
          : null,
  };
}

function isAskQuestionOption(value: unknown): value is AskQuestionOption {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.label === "string" &&
    (value.recommended === undefined || typeof value.recommended === "boolean")
  );
}

function isAskQuestion(value: unknown): value is AskQuestion {
  return (
    isRecord(value) &&
    (value.allowCustom === undefined || typeof value.allowCustom === "boolean") &&
    typeof value.id === "string" &&
    typeof value.prompt === "string" &&
    Array.isArray(value.options) &&
    value.options.every(isAskQuestionOption)
  );
}

function isAskQuestionAnswer(value: unknown): value is AskQuestionAnswer {
  return (
    isRecord(value) &&
    typeof value.questionId === "string" &&
    Array.isArray(value.optionIds) &&
    value.optionIds.every((entry) => typeof entry === "string") &&
    (value.customText === undefined ||
      value.customText === null ||
      typeof value.customText === "string") &&
    (value.skipped === undefined || typeof value.skipped === "boolean") &&
    typeof value.pickedRecommended === "boolean"
  );
}

export function isAskQuestionResult(value: unknown): value is AskQuestionResult {
  return (
    isRecord(value) &&
    Array.isArray(value.answers) &&
    value.answers.every(isAskQuestionAnswer) &&
    typeof value.cancelled === "boolean" &&
    (value.outcome === undefined ||
      value.outcome === "answered" ||
      value.outcome === "skipped" ||
      value.outcome === "interrupted" ||
      value.outcome === "host_disconnected" ||
      value.outcome === "cancelled_unknown")
  );
}

export function parseAskQuestionRequest(payload: unknown): AskQuestionWireRequest {
  if (!isRecord(payload)) {
    throw new Error("ask_question payload must be an object");
  }

  if (typeof payload.requestId !== "string") {
    throw new Error("ask_question payload is missing requestId");
  }

  if (typeof payload.responseEvent !== "string") {
    throw new Error("ask_question payload is missing responseEvent");
  }

  if (!Array.isArray(payload.questions) || !payload.questions.every(isAskQuestion)) {
    throw new Error("ask_question payload is missing questions");
  }

  if (
    (payload.sessionId !== undefined && typeof payload.sessionId !== "string") ||
    (payload.toolCallId !== undefined && typeof payload.toolCallId !== "string")
  ) {
    throw new Error("ask_question payload has invalid durable identity");
  }

  return {
    requestId: payload.requestId,
    responseEvent: payload.responseEvent,
    sessionId: payload.sessionId,
    toolCallId: payload.toolCallId,
    questions: payload.questions,
  };
}

export function isAskQuestionWireResponse(value: unknown): value is AskQuestionWireResponse {
  return (
    isRecord(value) &&
    typeof value.requestId === "string" &&
    isAskQuestionResult(value.result)
  );
}

export function normalizeAskQuestionResponse(
  requestId: string,
  payload: AskQuestionResult | AskQuestionWireResponse,
): AskQuestionWireResponse {
  if (isAskQuestionWireResponse(payload)) {
    if (payload.requestId !== requestId) {
      throw new Error("ask_question response requestId mismatch");
    }
    return payload;
  }

  if (!isAskQuestionResult(payload)) {
    throw new Error("invalid ask_question response payload");
  }

  return {
    requestId,
    result: payload,
  };
}

export function assertRequiredCapabilities(capabilities: string[]): void {
  const missing = REQUIRED_SERVE_CAPABILITIES.filter(
    (capability) => !capabilities.includes(capability),
  );
  if (missing.length > 0) {
    throw new Error(
      t("protocol.missingCapabilities", { capabilities: missing.join(", ") }),
    );
  }
}
