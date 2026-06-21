import * as vscode from "vscode";

import {
  TOMCAT_ANSWER_COMMAND,
  TOMCAT_APPLY_EDIT_COMMAND,
  TOMCAT_OPEN_DIFF_COMMAND,
} from "../../constants";
import type { InitializeResult } from "../../serveClient/initialize";
import {
  hasServeCapability,
  SERVE_CAPABILITY_LIST_MODELS,
  SERVE_CAPABILITY_SET_PLAN_MODE,
} from "../../serveClient/initialize";
import type {
  AskQuestionAnswer,
  AskQuestionResult,
  AskQuestionWireRequest,
  AskQuestionWireResponse,
  DisposableLike,
} from "../../serveClient/protocol";
import { normalizeAskQuestionResponse } from "../../serveClient/protocol";
import type { SessionRouter } from "../../serveClient/sessionRouter";
import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import type { VsCodeIde } from "../../ide/VsCodeIde";
import {
  normalizePlanState,
  planStateProgressLabel,
} from "./planState";

type TurnContext = {
  stream: vscode.ChatResponseStream;
};

type PendingQuestion = {
  request: AskQuestionWireRequest;
  resolve(response: AskQuestionWireResponse): void;
  sessionId?: string | null;
};

export interface PendingQuestionSnapshot {
  questions: AskQuestionWireRequest["questions"];
  requestId: string;
  sessionId?: string | null;
}

type AnswerCommandArgs =
  | {
      kind: "direct";
      optionId: string;
      pickedRecommended: boolean;
      questionId: string;
      requestId: string;
    }
  | {
      kind: "picker";
      requestId: string;
    }
  | {
      kind: "skip";
      questionId: string;
      requestId: string;
    };

type DiffCommandArgs = {
  toolCallId: string;
};

type AnswerQuickPickItem = vscode.QuickPickItem & {
  answerKind: "option" | "custom" | "skip";
  option?: AskQuestionWireRequest["questions"][number]["options"][number];
};

type ModelQuickPickItem = vscode.QuickPickItem & {
  isCurrent: boolean;
  modelId: string;
};

type PlanSlashCommand =
  | {
      action: "build";
      planId?: string;
    }
  | {
      action: "enter" | "exit";
    };

export interface SlashCommandContext {
  initializeResult: InitializeResult;
  messenger: TomcatMessenger;
  request: vscode.ChatRequest;
  sessionId: string;
  sessionRouter: SessionRouter;
  stream: vscode.ChatResponseStream;
}

export interface SlashCommandResult {
  awaitAgentEnd: boolean;
  error?: string;
}

type UiOverrides = {
  showInputBox?: (
    options: vscode.InputBoxOptions,
  ) => Thenable<string | undefined>;
  showQuickPick?: <T extends vscode.QuickPickItem>(
    items: readonly T[],
    options?: vscode.QuickPickOptions,
  ) => Thenable<T | undefined>;
};

function createDisposable(callback: () => void): DisposableLike {
  return {
    dispose: callback,
  };
}

function renderAskQuestionMarkdown(request: AskQuestionWireRequest): string {
  const lines = ["### Tomcat 需要你的确认", ""];
  for (const question of request.questions) {
    lines.push(`- **${question.prompt}**`);
    for (const option of question.options) {
      const suffix = option.recommended ? " (Recommended)" : "";
      lines.push(`  - ${option.label}${suffix}`);
    }
  }
  return lines.join("\n");
}

function toPendingQuestionSnapshot(
  request: AskQuestionWireRequest,
  sessionId?: string | null,
): PendingQuestionSnapshot {
  return {
    questions: request.questions,
    requestId: request.requestId,
    sessionId,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parsePlanSlashCommand(prompt: string): PlanSlashCommand | Error {
  const parts = prompt
    .trim()
    .split(/\s+/)
    .filter((part) => part.length > 0);

  if (parts.length === 0) {
    return { action: "enter" };
  }

  if (parts[0] === "enter" && parts.length === 1) {
    return { action: "enter" };
  }

  if (parts[0] === "exit" && parts.length === 1) {
    return { action: "exit" };
  }

  if (parts[0] === "build") {
    const planId = parts.slice(1).join(" ").trim();
    return {
      action: "build",
      planId: planId.length > 0 ? planId : undefined,
    };
  }

  return new Error(
    "Unknown `/plan` subcommand. Use `/plan`, `/plan exit`, or `/plan build [planId]`.",
  );
}

function parsePlanPayload(payload: unknown): {
  planId: string | null;
  planPath: string | null;
  planState: string | null;
} {
  if (!isRecord(payload)) {
    return {
      planId: null,
      planPath: null,
      planState: null,
    };
  }

  return {
    planId: typeof payload.planId === "string" ? payload.planId : null,
    planPath: typeof payload.planPath === "string" ? payload.planPath : null,
    planState: typeof payload.planState === "string" ? payload.planState : null,
  };
}

function parseModelEntries(payload: unknown): Array<{
  api: string | null;
  baseUrl: string | null;
  capabilities: string[];
  id: string;
  modelName: string | null;
  provider: string | null;
}> {
  if (!isRecord(payload) || !Array.isArray(payload.models)) {
    return [];
  }

  return payload.models
    .filter(isRecord)
    .map((entry) => ({
      api: typeof entry.api === "string" ? entry.api : null,
      baseUrl: typeof entry.baseUrl === "string" ? entry.baseUrl : null,
      capabilities: Array.isArray(entry.capabilities) &&
        entry.capabilities.every((item) => typeof item === "string")
          ? entry.capabilities
          : isRecord(entry.capabilities)
            ? Object.entries(entry.capabilities)
                .filter(([, enabled]) => enabled === true)
                .map(([name]) => name)
            : [],
      id: typeof entry.id === "string" ? entry.id : "",
      modelName:
        typeof entry.modelName === "string" ? entry.modelName : null,
      provider: typeof entry.provider === "string" ? entry.provider : null,
    }))
    .filter((entry) => entry.id.length > 0);
}

function toModelQuickPickItems(
  entries: ReturnType<typeof parseModelEntries>,
  currentModel: string | null | undefined,
): ModelQuickPickItem[] {
  return [...entries]
    .sort((left, right) => {
      if (left.id === currentModel && right.id !== currentModel) {
        return -1;
      }
      if (left.id !== currentModel && right.id === currentModel) {
        return 1;
      }
      return left.id.localeCompare(right.id);
    })
    .map((entry) => {
      const isCurrent = entry.id === currentModel;
      const descriptionParts = [
        isCurrent ? "Current" : null,
        entry.provider,
        entry.modelName && entry.modelName !== entry.id ? entry.modelName : null,
      ].filter((part): part is string => !!part);
      const detailParts = [
        entry.api,
        entry.baseUrl,
        entry.capabilities.length > 0
          ? `caps: ${entry.capabilities.join(", ")}`
          : null,
      ].filter((part): part is string => !!part);

      return {
        description:
          descriptionParts.length > 0 ? descriptionParts.join(" · ") : undefined,
        detail: detailParts.length > 0 ? detailParts.join(" · ") : undefined,
        isCurrent,
        label: entry.id,
        modelId: entry.id,
      };
    });
}

export class ParticipantCommands {
  private readonly activeTurns = new Map<string, TurnContext>();
  private readonly pendingQuestions = new Map<string, PendingQuestion>();
  private readonly pendingQuestionListeners = new Set<
    (question: PendingQuestionSnapshot) => void
  >();
  private uiOverrides: UiOverrides = {};

  constructor(private readonly ide: VsCodeIde) {}

  register(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
      vscode.commands.registerCommand(
        TOMCAT_ANSWER_COMMAND,
        async (args: AnswerCommandArgs) => {
          await this.handleAnswerCommand(args);
        },
      ),
      vscode.commands.registerCommand(
        TOMCAT_OPEN_DIFF_COMMAND,
        async (args: DiffCommandArgs) => {
          await this.ide.openPreparedDiff(args.toolCallId);
        },
      ),
      vscode.commands.registerCommand(
        TOMCAT_APPLY_EDIT_COMMAND,
        async (args: DiffCommandArgs) => {
          const applied = await this.ide.applyPreparedEdit(args.toolCallId);
          if (applied) {
            await vscode.window.showInformationMessage("Tomcat edit applied.");
            return;
          }
          await vscode.window.showWarningMessage("Tomcat edit could not be applied.");
        },
      ),
    );
  }

  attachTurn(sessionId: string, stream: vscode.ChatResponseStream): DisposableLike {
    const turn: TurnContext = { stream };
    this.activeTurns.set(sessionId, turn);
    return createDisposable(() => {
      if (this.activeTurns.get(sessionId) === turn) {
        this.activeTurns.delete(sessionId);
      }
    });
  }

  setUiOverrides(overrides: UiOverrides): void {
    this.uiOverrides = overrides;
  }

  getPendingQuestion(requestId?: string): PendingQuestionSnapshot | undefined {
    if (requestId) {
      const pending = this.pendingQuestions.get(requestId);
      return pending ? toPendingQuestionSnapshot(pending.request, pending.sessionId) : undefined;
    }

    const firstPending = this.pendingQuestions.values().next().value as PendingQuestion | undefined;
    return firstPending
      ? toPendingQuestionSnapshot(firstPending.request, firstPending.sessionId)
      : undefined;
  }

  onPendingQuestion(
    listener: (question: PendingQuestionSnapshot) => void,
  ): DisposableLike {
    this.pendingQuestionListeners.add(listener);
    return createDisposable(() => {
      this.pendingQuestionListeners.delete(listener);
    });
  }

  async handleModelSlashCommand(
    context: SlashCommandContext,
  ): Promise<SlashCommandResult> {
    if (
      !hasServeCapability(context.initializeResult, SERVE_CAPABILITY_LIST_MODELS)
    ) {
      context.stream.markdown(
        "Tomcat `/model` is unavailable because the connected `tomcat serve` does not support `list_models` yet.",
      );
      return { awaitAgentEnd: false };
    }

    const currentState = await context.sessionRouter
      .getState(context.sessionId)
      .catch(() => null);
    const listResponse = await context.messenger.sendListModels();
    if (!listResponse.success) {
      return {
        awaitAgentEnd: false,
        error: `Tomcat /model failed: ${listResponse.error ?? "unable to list models"}`,
      };
    }

    const items = toModelQuickPickItems(
      parseModelEntries(listResponse.payload),
      currentState?.model,
    );
    if (items.length === 0) {
      context.stream.markdown(
        "Tomcat did not report any configured models for `/model`.",
      );
      return { awaitAgentEnd: false };
    }

    const picked = await this.showQuickPick<ModelQuickPickItem>(items, {
      ignoreFocusOut: true,
      placeHolder: "Select a Tomcat model",
      title: "Tomcat model",
    });
    if (!picked) {
      return { awaitAgentEnd: false };
    }

    const setResponse = await context.messenger.sendSetModel(
      context.sessionId,
      picked.modelId,
    );
    if (!setResponse.success) {
      return {
        awaitAgentEnd: false,
        error: `Tomcat /model failed: ${setResponse.error ?? "unable to switch model"}`,
      };
    }

    const confirmedState = await context.sessionRouter
      .getState(context.sessionId)
      .catch(() => currentState);
    const activeModel = confirmedState?.model ?? picked.modelId;
    context.stream.markdown(`Switched Tomcat model to \`${activeModel}\`.`);

    return { awaitAgentEnd: false };
  }

  async handlePlanSlashCommand(
    context: SlashCommandContext,
  ): Promise<SlashCommandResult> {
    if (
      !hasServeCapability(context.initializeResult, SERVE_CAPABILITY_SET_PLAN_MODE)
    ) {
      context.stream.markdown(
        "Tomcat `/plan` is unavailable because the connected `tomcat serve` does not support `set_plan_mode` yet.",
      );
      return { awaitAgentEnd: false };
    }

    const parsedCommand = parsePlanSlashCommand(context.request.prompt);
    if (parsedCommand instanceof Error) {
      return {
        awaitAgentEnd: false,
        error: parsedCommand.message,
      };
    }

    const response = await context.messenger.sendSetPlanMode({
      action: parsedCommand.action,
      planId:
        parsedCommand.action === "build" ? parsedCommand.planId : undefined,
      sessionId: context.sessionId,
    });
    if (!response.success) {
      return {
        awaitAgentEnd: false,
        error: `Tomcat /plan failed: ${response.error ?? "unable to update plan mode"}`,
      };
    }

    const responsePayload = parsePlanPayload(response.payload);
    const confirmedState = await context.sessionRouter
      .getState(context.sessionId)
      .catch(() => null);
    const planState = normalizePlanState(
      responsePayload.planState ?? confirmedState?.planState,
    );
    const planId = responsePayload.planId ?? confirmedState?.planId;

    context.stream.progress(planStateProgressLabel(planState, planId));

    if (parsedCommand.action === "build") {
      const target = responsePayload.planPath ?? planId ?? "the active plan";
      context.stream.markdown(`Started building \`${target}\`.`);
      return { awaitAgentEnd: true };
    }

    if (parsedCommand.action === "exit") {
      context.stream.markdown("Tomcat exited plan mode.");
      return { awaitAgentEnd: false };
    }

    context.stream.markdown("Tomcat entered plan mode.");
    return { awaitAgentEnd: false };
  }

  async askUser(
    request: AskQuestionWireRequest,
    sessionId?: string | null,
  ): Promise<AskQuestionWireResponse> {
    const turn = sessionId ? this.activeTurns.get(sessionId) : undefined;
    if (turn) {
      turn.stream.markdown(renderAskQuestionMarkdown(request));
    }

    const responsePromise = new Promise<AskQuestionWireResponse>((resolve) => {
      this.pendingQuestions.set(request.requestId, {
        request,
        resolve,
        sessionId,
      });
      const snapshot = toPendingQuestionSnapshot(request, sessionId);
      for (const listener of this.pendingQuestionListeners) {
        listener(snapshot);
      }
    }).finally(() => {
      this.pendingQuestions.delete(request.requestId);
    });

    if (!turn) {
      return normalizeAskQuestionResponse(
        request.requestId,
        await this.collectAnswersWithQuickPick(request),
      );
    }

    if (request.questions.length === 1) {
      const question = request.questions[0];
      for (const option of question.options) {
        turn.stream.button({
          arguments: [
            {
              kind: "direct",
              optionId: option.id,
              pickedRecommended: !!option.recommended,
              questionId: question.id,
              requestId: request.requestId,
            } satisfies AnswerCommandArgs,
          ],
          command: TOMCAT_ANSWER_COMMAND,
          title: option.label,
        });
      }
      turn.stream.button({
        arguments: [{ kind: "skip", questionId: question.id, requestId: request.requestId } satisfies AnswerCommandArgs],
        command: TOMCAT_ANSWER_COMMAND,
        title: "Skip",
      });
    }

    turn.stream.button({
      arguments: [{ kind: "picker", requestId: request.requestId } satisfies AnswerCommandArgs],
      command: TOMCAT_ANSWER_COMMAND,
      title: request.questions.length === 1 ? "Other..." : "Answer Questions",
    });

    return responsePromise;
  }

  private async handleAnswerCommand(args: AnswerCommandArgs): Promise<void> {
    const pending = this.pendingQuestions.get(args.requestId);
    if (!pending) {
      await vscode.window.showWarningMessage(
        "This Tomcat approval prompt is no longer active.",
      );
      return;
    }

    let result: AskQuestionResult;
    if (args.kind === "direct") {
      result = {
        answers: [
          {
            customText: null,
            optionIds: [args.optionId],
            pickedRecommended: args.pickedRecommended,
            questionId: args.questionId,
            skipped: false,
          },
        ],
        cancelled: false,
      };
    } else if (args.kind === "skip") {
      result = {
        answers: [
          {
            customText: null,
            optionIds: [],
            pickedRecommended: false,
            questionId: args.questionId,
            skipped: true,
          },
        ],
        cancelled: false,
      };
    } else {
      result = await this.collectAnswersWithQuickPick(pending.request);
    }

    pending.resolve(normalizeAskQuestionResponse(pending.request.requestId, result));
  }

  private async collectAnswersWithQuickPick(
    request: AskQuestionWireRequest,
  ): Promise<AskQuestionResult> {
    const answers: AskQuestionAnswer[] = [];

    for (const question of request.questions) {
      const selection = await this.showQuickPick<AnswerQuickPickItem>(
        [
          ...question.options.map((option) => ({
            description: option.recommended ? "Recommended" : undefined,
            answerKind: "option" as const,
            option,
            label: option.label,
          })),
          {
            answerKind: "custom" as const,
            label: "Other...",
          },
          {
            answerKind: "skip" as const,
            label: "Skip",
          },
        ],
        {
          ignoreFocusOut: true,
          placeHolder: question.prompt,
          title: "Tomcat approval",
        },
      );

      if (!selection) {
        return { answers: [], cancelled: true };
      }

      if (selection.answerKind === "skip") {
        answers.push({
          customText: null,
          optionIds: [],
          pickedRecommended: false,
          questionId: question.id,
          skipped: true,
        });
        continue;
      }

      if (selection.answerKind === "custom") {
        const customText = await this.showInputBox({
          ignoreFocusOut: true,
          prompt: question.prompt,
          title: "Tomcat custom answer",
        });

        if (!customText) {
          return { answers: [], cancelled: true };
        }

        answers.push({
          customText,
          optionIds: ["__custom__"],
          pickedRecommended: false,
          questionId: question.id,
          skipped: false,
        });
        continue;
      }

      answers.push({
        customText: null,
        optionIds: [selection.option!.id],
        pickedRecommended: !!selection.option!.recommended,
        questionId: question.id,
        skipped: false,
      });
    }

    return {
      answers,
      cancelled: false,
    };
  }

  private showInputBox(
    options: vscode.InputBoxOptions,
  ): Thenable<string | undefined> {
    return this.uiOverrides.showInputBox?.(options) ?? vscode.window.showInputBox(options);
  }

  private showQuickPick<T extends vscode.QuickPickItem>(
    items: readonly T[],
    options?: vscode.QuickPickOptions,
  ): Thenable<T | undefined> {
    return (
      this.uiOverrides.showQuickPick?.(items, options) ??
      vscode.window.showQuickPick(items, options)
    );
  }
}
