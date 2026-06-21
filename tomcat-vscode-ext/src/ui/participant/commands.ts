import * as vscode from "vscode";

import {
  TOMCAT_ANSWER_COMMAND,
  TOMCAT_APPLY_EDIT_COMMAND,
  TOMCAT_OPEN_DIFF_COMMAND,
} from "../../constants";
import type {
  AskQuestionAnswer,
  AskQuestionResult,
  AskQuestionWireRequest,
  AskQuestionWireResponse,
  DisposableLike,
} from "../../serveClient/protocol";
import { normalizeAskQuestionResponse } from "../../serveClient/protocol";
import type { VsCodeIde } from "../../ide/VsCodeIde";

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

export class ParticipantCommands {
  private readonly activeTurns = new Map<string, TurnContext>();
  private readonly pendingQuestions = new Map<string, PendingQuestion>();
  private readonly pendingQuestionListeners = new Set<
    (question: PendingQuestionSnapshot) => void
  >();

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
      const selection = await vscode.window.showQuickPick<AnswerQuickPickItem>(
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
        const customText = await vscode.window.showInputBox({
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
}
