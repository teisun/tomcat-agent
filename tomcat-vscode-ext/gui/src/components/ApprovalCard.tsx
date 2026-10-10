import { memo, useEffect, useId, useRef, useState } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey } from "../../../src/shared/i18n";

import {
  CUSTOM_OPTION_ID,
  type AskQuestionResult,
  type WebviewApprovalCard,
  type WebviewApprovalOption,
} from "../types";

export type ApprovalQuestionDraft = {
  customText: string;
  optionId: string | null;
};

export type ApprovalAnswerDraft = Record<string, ApprovalQuestionDraft>;

export type ApprovalAnswerState = {
  draft: ApprovalAnswerDraft;
  submitting: boolean;
};

export function approvalAnswerKey(sessionId: string, requestId: string): string {
  return `${sessionId}\u0000${requestId}`;
}

export function createApprovalAnswerDraft(item: WebviewApprovalCard): ApprovalAnswerDraft {
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

function buildOptionCode(index: number): string {
  let remaining = index;
  let code = "";

  do {
    code = String.fromCharCode(65 + (remaining % 26)) + code;
    remaining = Math.floor(remaining / 26) - 1;
  } while (remaining >= 0);

  return code;
}

type ApprovalCardPresentation = "default" | "pending";

function ApprovalCardComponent({
  collapsed = false,
  draft: suppliedDraft,
  item,
  onAnswer,
  onCollapsedChange,
  onDraftChange,
  pendingGroupCount = 1,
  presentation = "default",
  submitting = false,
}: {
  collapsed?: boolean;
  draft?: ApprovalAnswerDraft;
  item: WebviewApprovalCard;
  onAnswer(sessionId: string, requestId: string, result: AskQuestionResult): void;
  onCollapsedChange?(collapsed: boolean): void;
  onDraftChange(sessionId: string, requestId: string, draft: ApprovalAnswerDraft): void;
  pendingGroupCount?: number;
  presentation?: ApprovalCardPresentation;
  submitting?: boolean;
}) {
  const t = useT();
  const locale = useLocale();
  const draft = suppliedDraft ?? createApprovalAnswerDraft(item);
  const isPendingPanel = presentation === "pending";
  const questions = item.request.questions;
  const [activeQuestionIndex, setActiveQuestionIndex] = useState(0);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const collapseButtonRef = useRef<HTMLButtonElement | null>(null);
  const bodyId = `approval-questions-${useId().replaceAll(":", "")}`;

  useEffect(() => {
    setActiveQuestionIndex(0);
  }, [item.request.requestId]);

  const showQuestion = (index: number) => {
    setActiveQuestionIndex(Math.max(0, Math.min(index, questions.length - 1)));
  };

  const toggleCollapsed = () => {
    // Move focus before the panel body is unmounted so a focused option never disappears
    // from the keyboard sequence when the user folds the dock.
    if (!collapsed && bodyRef.current?.contains(document.activeElement)) {
      collapseButtonRef.current?.focus();
    }
    onCollapsedChange?.(!collapsed);
  };

  if (item.resolved) {
    return null;
  }
  if (!item.live) {
    return (
      <section
        aria-live="polite"
        className="tc-card tc-approval-card tc-approval-card--restoring"
        data-testid="approval-card-restoring"
      >
        <div className="tc-card__header">
          <h3>{t("question.pending")}</h3>
        </div>
        <p>{t("question.restoring")}</p>
      </section>
    );
  }

  const canContinue = questions.every((question) => {
    const questionDraft = draft[question.id];
    if (!questionDraft?.optionId) {
      return false;
    }
    if (questionDraft.optionId !== CUSTOM_OPTION_ID) {
      return question.options.some(option => option.id === questionDraft.optionId);
    }
    return question.allowCustom !== false && questionDraft.customText.trim().length > 0;
  });

  const selectOption = (questionId: string, optionId: string) => {
    if (submitting) return;
    const current = draft[questionId] ?? { customText: "", optionId: null };
    onDraftChange(item.sessionId ?? "", item.request.requestId, {
      ...draft,
      [questionId]: {
        ...current,
        optionId,
      },
    });
  };

  const updateCustomText = (questionId: string, customText: string) => {
    if (submitting) return;
    const current = draft[questionId] ?? { customText: "", optionId: CUSTOM_OPTION_ID };
    onDraftChange(item.sessionId ?? "", item.request.requestId, {
      ...draft,
      [questionId]: {
        ...current,
        customText,
      },
    });
  };

  const submitAnswers = () => {
    if (!canContinue || submitting) {
      return;
    }

    onAnswer(item.sessionId ?? "", item.request.requestId, {
      answers: questions.map((question) => {
        const questionDraft = draft[question.id];
        const optionId = questionDraft?.optionId;
        if (!optionId) {
          throw new Error(`missing approval answer for question ${question.id}`);
        }
        if (optionId === CUSTOM_OPTION_ID) {
          return {
            customText: questionDraft.customText.trim(),
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
      outcome: "answered",
    });
  };

  const skipQuestions = () => {
    if (submitting) return;
    onAnswer(item.sessionId ?? "", item.request.requestId, {
      answers: [],
      cancelled: true,
      outcome: "skipped",
    });
  };

  const hasPreviousQuestion = activeQuestionIndex > 0;
  const hasNextQuestion = activeQuestionIndex < questions.length - 1;
  const extraGroupCount = Math.max(0, pendingGroupCount - 1);

  return (
    <section
      aria-busy={submitting || undefined}
      className={`tc-card tc-approval-card${isPendingPanel ? " tc-approval-card--pending" : ""}`}
      data-testid="approval-card"
    >
      <div className="tc-card__header tc-approval-card__header">
        <h3>
          {isPendingPanel ? <span aria-hidden="true" className="codicon codicon-question" /> : null}
          {t("question.title")}
        </h3>
        {isPendingPanel ? (
          <div className="tc-approval-card__navigation">
            {extraGroupCount > 0 ? (
              <span className="tc-approval-card__more-groups">
                {t(pluralKey(locale, "question.groups.other", extraGroupCount), { count: extraGroupCount })}
              </span>
            ) : null}
            {!collapsed ? (
              <button
                aria-label={t("question.previous")}
                className="tc-approval-card__nav-button"
                data-testid="approval-previous-question"
                disabled={!hasPreviousQuestion}
                onClick={() => showQuestion(activeQuestionIndex - 1)}
                title={t("question.previous")}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-chevron-left" />
              </button>
            ) : null}
            <span
              aria-live="polite"
              className="tc-approval-card__count"
              data-testid="approval-question-count"
            >
              {t("question.position", { current: questions.length === 0 ? 0 : activeQuestionIndex + 1, total: questions.length })}
            </span>
            {!collapsed ? (
              <button
                aria-label={t("question.next")}
                className="tc-approval-card__nav-button"
                data-testid="approval-next-question"
                disabled={!hasNextQuestion}
                onClick={() => showQuestion(activeQuestionIndex + 1)}
                title={t("question.next")}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-chevron-right" />
              </button>
            ) : null}
            <button
              aria-controls={bodyId}
              aria-expanded={!collapsed}
              aria-label={t(collapsed ? "question.expand" : "question.collapse")}
              className="tc-approval-card__collapse"
              data-testid="approval-collapse"
              onClick={toggleCollapsed}
              ref={collapseButtonRef}
              title={t(collapsed ? "question.expand" : "question.collapse")}
              type="button"
            >
              <span
                aria-hidden="true"
                className={`codicon ${collapsed ? "codicon-chevron-right" : "codicon-chevron-down"}`}
              />
            </button>
          </div>
        ) : (
          <span className="tc-chip tc-chip--warning">{t("question.position", { current: questions.length, total: questions.length })}</span>
        )}
      </div>

      {!isPendingPanel || !collapsed ? (
        <div
          className={`tc-approval-questions${isPendingPanel ? " tc-approval-questions--pending" : ""}`}
          data-testid={isPendingPanel ? "approval-questions-body" : undefined}
          id={isPendingPanel ? bodyId : undefined}
          key={isPendingPanel ? activeQuestionIndex : "all"}
          ref={isPendingPanel ? bodyRef : undefined}
        >
          {questions.map((question, questionIndex) => {
            // Only the current page is mounted; answers remain in the shared draft.
            if (isPendingPanel && questionIndex !== activeQuestionIndex) return null;
            const questionDraft = draft[question.id] ?? { customText: "", optionId: null };
            const options: WebviewApprovalOption[] = [
              ...question.options,
              ...(question.allowCustom === false ? [] : [{ id: CUSTOM_OPTION_ID, label: t("question.other") }]),
            ];

            return (
              <div
                className="tc-approval-question"
                key={question.id}
              >
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
                    const selected = questionDraft.optionId === option.id;
                    return (
                      <button
                        aria-checked={selected}
                        aria-label={option.recommended ? t("question.recommendedOption", { label: option.label }) : option.label}
                        className={
                          selected
                            ? "tc-approval-option tc-approval-option--selected"
                            : "tc-approval-option"
                        }
                        data-testid={`approval-option-${question.id}-${option.id}`}
                        disabled={submitting}
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
                            <span className="tc-approval-option__recommended">{t("question.recommended")}</span>
                          ) : null}
                        </span>
                      </button>
                    );
                  })}
                </div>
                {question.allowCustom !== false && questionDraft.optionId === CUSTOM_OPTION_ID ? (
                  <label className="tc-field tc-approval-custom">
                    <span>{t("question.custom")}</span>
                    <input
                      autoFocus={isPendingPanel}
                      className="tc-approval-custom__input"
                      data-testid={`approval-custom-${question.id}`}
                      disabled={submitting}
                      onChange={(event) => updateCustomText(question.id, event.target.value)}
                      placeholder={t("question.customPlaceholder")}
                      type="text"
                      value={questionDraft.customText}
                    />
                  </label>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : null}

      {!isPendingPanel || !collapsed ? (
        <div className="tc-approval-actions">
          <button
            className="tc-button tc-button--ghost"
            data-testid="approval-skip"
            disabled={submitting}
            onClick={skipQuestions}
            type="button"
          >
            {t("question.skip")}
          </button>
          <button
            className="tc-button tc-button--primary"
            data-testid="approval-continue"
            disabled={!canContinue || submitting}
            onClick={submitAnswers}
            type="button"
          >
            {t(submitting ? "question.submitting" : "question.continue")}
          </button>
        </div>
      ) : null}
    </section>
  );
}

function areApprovalCardPropsEqual(
  previous: Readonly<Parameters<typeof ApprovalCardComponent>[0]>,
  next: Readonly<Parameters<typeof ApprovalCardComponent>[0]>,
): boolean {
  return (
    previous.item === next.item
    && previous.draft === next.draft
    && previous.submitting === next.submitting
    && previous.presentation === next.presentation
    && previous.collapsed === next.collapsed
    && previous.pendingGroupCount === next.pendingGroupCount
    && previous.onAnswer === next.onAnswer
    && previous.onCollapsedChange === next.onCollapsedChange
    && previous.onDraftChange === next.onDraftChange
  );
}

export const ApprovalCard = memo(ApprovalCardComponent, areApprovalCardPropsEqual);
