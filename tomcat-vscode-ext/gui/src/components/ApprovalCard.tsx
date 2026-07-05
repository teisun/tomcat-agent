import { useState } from "react";

import {
  CUSTOM_OPTION_ID,
  type AskQuestionResult,
  type WebviewApprovalCard,
  type WebviewApprovalOption,
} from "../types";

type QuestionDraft = {
  customText: string;
  optionId: string | null;
};

function buildOptionCode(index: number): string {
  let remaining = index;
  let code = "";

  do {
    code = String.fromCharCode(65 + (remaining % 26)) + code;
    remaining = Math.floor(remaining / 26) - 1;
  } while (remaining >= 0);

  return code;
}

function createInitialDrafts(item: WebviewApprovalCard): Record<string, QuestionDraft> {
  return Object.fromEntries(
    item.request.questions.map((question) => [
      question.id,
      {
        customText: "",
        optionId: null,
      },
    ]),
  );
}

export function ApprovalCard({
  item,
  onAnswer,
}: {
  item: WebviewApprovalCard;
  onAnswer(requestId: string, result: AskQuestionResult): void;
}) {
  const [drafts, setDrafts] = useState<Record<string, QuestionDraft>>(() => createInitialDrafts(item));

  if (item.resolved) {
    return null;
  }

  const canContinue = item.request.questions.every((question) => {
    const draft = drafts[question.id];
    if (!draft?.optionId) {
      return false;
    }
    if (draft.optionId !== CUSTOM_OPTION_ID) {
      return true;
    }
    return draft.customText.trim().length > 0;
  });

  const selectOption = (questionId: string, optionId: string) => {
    setDrafts((current) => ({
      ...current,
      [questionId]: {
        ...(current[questionId] ?? { customText: "", optionId: null }),
        optionId,
      },
    }));
  };

  const updateCustomText = (questionId: string, customText: string) => {
    setDrafts((current) => ({
      ...current,
      [questionId]: {
        ...(current[questionId] ?? { customText: "", optionId: CUSTOM_OPTION_ID }),
        customText,
      },
    }));
  };

  const submitAnswers = () => {
    if (!canContinue) {
      return;
    }

    onAnswer(item.request.requestId, {
      answers: item.request.questions.map((question) => {
        const draft = drafts[question.id];
        const optionId = draft?.optionId;
        if (!optionId) {
          throw new Error(`missing approval answer for question ${question.id}`);
        }
        if (optionId === CUSTOM_OPTION_ID) {
          return {
            customText: draft.customText.trim(),
            optionIds: [CUSTOM_OPTION_ID],
            pickedRecommended: false,
            questionId: question.id,
          };
        }

        const selectedOption = question.options.find((option) => option.id === optionId);
        return {
          optionIds: [optionId],
          pickedRecommended: !!selectedOption?.recommended,
          questionId: question.id,
        };
      }),
      cancelled: false,
    });
  };

  const skipQuestions = () => {
    onAnswer(item.request.requestId, {
      answers: [],
      cancelled: true,
    });
  };

  return (
    <section className="tc-card tc-approval-card" data-testid="approval-card">
      <div className="tc-card__header">
        <h3>Questions</h3>
        <span className="tc-chip tc-chip--warning">{item.request.questions.length} of {item.request.questions.length}</span>
      </div>
      <div className="tc-approval-questions">
        {item.request.questions.map((question, questionIndex) => {
          const draft = drafts[question.id] ?? { customText: "", optionId: null };
          const options: WebviewApprovalOption[] = [
            ...question.options,
            { id: CUSTOM_OPTION_ID, label: "Other..." },
          ];

          return (
            <div className="tc-approval-question" key={question.id}>
              <div className="tc-approval-question__prompt">
                <span className="tc-approval-question__index">{questionIndex + 1}.</span>
                <p>{question.prompt}</p>
              </div>
              <div
                aria-label={question.prompt}
                className="tc-approval-options"
                role="radiogroup"
              >
                {options.map((option, optionIndex) => {
                  const selected = draft.optionId === option.id;
                  return (
                    <button
                      aria-checked={selected}
                      className={
                        selected
                          ? "tc-approval-option tc-approval-option--selected"
                          : "tc-approval-option"
                      }
                      data-testid={`approval-option-${question.id}-${option.id}`}
                      key={option.id}
                      onClick={() => selectOption(question.id, option.id)}
                      role="radio"
                      type="button"
                    >
                      <span
                        aria-hidden="true"
                        className={
                          selected
                            ? "tc-approval-option__code tc-approval-option__code--selected"
                            : "tc-approval-option__code"
                        }
                      >
                        {buildOptionCode(optionIndex)}
                      </span>
                      <span className="tc-approval-option__content">
                        <span className="tc-approval-option__label">{option.label}</span>
                        {option.recommended ? (
                          <span className="tc-approval-option__recommended">Recommended</span>
                        ) : null}
                      </span>
                    </button>
                  );
                })}
              </div>
              {draft.optionId === CUSTOM_OPTION_ID ? (
                <label className="tc-field tc-approval-custom">
                  <span>Custom answer</span>
                  <input
                    className="tc-approval-custom__input"
                    data-testid={`approval-custom-${question.id}`}
                    onChange={(event) => updateCustomText(question.id, event.target.value)}
                    placeholder="Enter a custom answer"
                    type="text"
                    value={draft.customText}
                  />
                </label>
              ) : null}
            </div>
          );
        })}
      </div>
      <div className="tc-approval-actions">
        <button
          className="tc-button tc-button--ghost"
          data-testid="approval-skip"
          onClick={skipQuestions}
          type="button"
        >
          Skip
        </button>
        <button
          className="tc-button tc-button--primary"
          data-testid="approval-continue"
          disabled={!canContinue}
          onClick={submitAnswers}
          type="button"
        >
          Continue
        </button>
      </div>
    </section>
  );
}
