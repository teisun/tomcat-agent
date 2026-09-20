import { memo, useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";

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
  const draft = suppliedDraft ?? createApprovalAnswerDraft(item);
  const isPendingPanel = presentation === "pending";
  const questions = item.request.questions;
  const [activeQuestionIndex, setActiveQuestionIndex] = useState(0);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const collapseButtonRef = useRef<HTMLButtonElement | null>(null);
  const questionElementsRef = useRef<Array<HTMLDivElement | null>>([]);
  const bodyId = `approval-questions-${useId().replaceAll(":", "")}`;

  useEffect(() => {
    setActiveQuestionIndex(0);
  }, [item.request.requestId]);

  const updateActiveQuestionFromScroll = useCallback(() => {
    const body = bodyRef.current;
    if (!body || questions.length === 0) {
      return;
    }
    if (
      body.scrollHeight > body.clientHeight
      && body.scrollTop + body.clientHeight >= body.scrollHeight - 1
    ) {
      setActiveQuestionIndex(questions.length - 1);
      return;
    }
    const bodyRect = body.getBoundingClientRect();
    setActiveQuestionIndex((current) => {
      let bestIndex = current;
      let bestVisible = 0;
      for (let index = 0; index < questions.length; index += 1) {
        const element = questionElementsRef.current[index];
        if (!element) continue;
        const rect = element.getBoundingClientRect();
        const visible = Math.max(
          0,
          Math.min(rect.bottom, bodyRect.bottom) - Math.max(rect.top, bodyRect.top),
        );
        // Keeping the previous index for ties stops the count from flickering while two
        // neighbouring questions are equally visible.
        if (visible > bestVisible) {
          bestIndex = index;
          bestVisible = visible;
        }
      }
      return bestIndex;
    });
  }, [questions.length]);

  const showQuestion = useCallback((index: number) => {
    const nextIndex = Math.max(0, Math.min(index, questions.length - 1));
    const body = bodyRef.current;
    const target = questionElementsRef.current[nextIndex];
    if (body && target) {
      const top = Math.max(0, target.offsetTop - body.offsetTop);
      if (typeof body.scrollTo === "function") {
        body.scrollTo({ behavior: "smooth", top });
      } else {
        body.scrollTop = top;
      }
    }
    setActiveQuestionIndex(nextIndex);
  }, [questions.length]);

  const toggleCollapsed = () => {
    // Move focus before the panel body is unmounted so a focused option never disappears
    // from the keyboard sequence when the user folds the dock.
    if (!collapsed && bodyRef.current?.contains(document.activeElement)) {
      collapseButtonRef.current?.focus();
    }
    onCollapsedChange?.(!collapsed);
  };

  // Reopening the dock creates a fresh scroll container. Restore the question that
  // was active when it collapsed before the scroll observer derives a new count.
  useLayoutEffect(() => {
    if (!isPendingPanel || collapsed) {
      return;
    }
    const body = bodyRef.current;
    const target = questionElementsRef.current[activeQuestionIndex];
    if (!body || !target) {
      return;
    }
    body.scrollTop = Math.max(0, target.offsetTop - body.offsetTop);
  }, [collapsed, isPendingPanel, item.request.requestId]);

  useEffect(() => {
    if (!isPendingPanel || collapsed) {
      return;
    }
    const body = bodyRef.current;
    if (!body) return;
    const observe = () => updateActiveQuestionFromScroll();
    window.addEventListener("resize", observe);
    const resizeObserver = typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(observe);
    resizeObserver?.observe(body);
    questionElementsRef.current.forEach((element) => element && resizeObserver?.observe(element));
    observe();
    return () => {
      window.removeEventListener("resize", observe);
      resizeObserver?.disconnect();
    };
  }, [collapsed, isPendingPanel, suppliedDraft, updateActiveQuestionFromScroll]);

  useEffect(() => {
    if (
      !isPendingPanel
      || !collapsed
      || !bodyRef.current?.contains(document.activeElement)
    ) {
      return;
    }
    collapseButtonRef.current?.focus();
  }, [collapsed, isPendingPanel]);

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
          <h3>Question pending</h3>
        </div>
        <p>Restoring this question…</p>
      </section>
    );
  }

  const canContinue = questions.every((question) => {
    const questionDraft = draft[question.id];
    if (!questionDraft?.optionId) {
      return false;
    }
    if (questionDraft.optionId !== CUSTOM_OPTION_ID) {
      return true;
    }
    return questionDraft.customText.trim().length > 0;
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
        <h3>Questions</h3>
        {isPendingPanel ? (
          <div className="tc-approval-card__navigation">
            {extraGroupCount > 0 ? (
              <span className="tc-approval-card__more-groups">
                {extraGroupCount} more question group{extraGroupCount === 1 ? "" : "s"} waiting
              </span>
            ) : null}
            {!collapsed ? (
              <button
                aria-label="Previous question"
                className="tc-approval-card__nav-button"
                data-testid="approval-previous-question"
                disabled={!hasPreviousQuestion}
                onClick={() => showQuestion(activeQuestionIndex - 1)}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-chevron-up" />
              </button>
            ) : null}
            <span
              aria-live="polite"
              className="tc-chip tc-chip--warning"
              data-testid="approval-question-count"
            >
              {questions.length === 0 ? 0 : activeQuestionIndex + 1} of {questions.length}
            </span>
            {!collapsed ? (
              <button
                aria-label="Next question"
                className="tc-approval-card__nav-button"
                data-testid="approval-next-question"
                disabled={!hasNextQuestion}
                onClick={() => showQuestion(activeQuestionIndex + 1)}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-chevron-down" />
              </button>
            ) : null}
            <button
              aria-controls={bodyId}
              aria-expanded={!collapsed}
              aria-label={collapsed ? "Expand questions" : "Collapse questions"}
              className="tc-approval-card__collapse"
              data-testid="approval-collapse"
              onClick={toggleCollapsed}
              ref={collapseButtonRef}
              title={collapsed ? "Expand questions" : "Collapse questions"}
              type="button"
            >
              <span
                aria-hidden="true"
                className={`codicon ${collapsed ? "codicon-chevron-right" : "codicon-chevron-down"}`}
              />
            </button>
          </div>
        ) : (
          <span className="tc-chip tc-chip--warning">{questions.length} of {questions.length}</span>
        )}
      </div>

      {!isPendingPanel || !collapsed ? (
        <div
          className={`tc-approval-questions${isPendingPanel ? " tc-approval-questions--pending" : ""}`}
          data-testid={isPendingPanel ? "approval-questions-body" : undefined}
          id={isPendingPanel ? bodyId : undefined}
          onScroll={isPendingPanel ? updateActiveQuestionFromScroll : undefined}
          ref={isPendingPanel ? bodyRef : undefined}
        >
          {questions.map((question, questionIndex) => {
            const questionDraft = draft[question.id] ?? { customText: "", optionId: null };
            const options: WebviewApprovalOption[] = [
              ...question.options,
              { id: CUSTOM_OPTION_ID, label: "Other..." },
            ];

            return (
              <div
                className="tc-approval-question"
                key={question.id}
                ref={isPendingPanel
                  ? (element) => { questionElementsRef.current[questionIndex] = element; }
                  : undefined}
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
                            <span className="tc-approval-option__recommended">Recommended</span>
                          ) : null}
                        </span>
                      </button>
                    );
                  })}
                </div>
                {questionDraft.optionId === CUSTOM_OPTION_ID ? (
                  <label className="tc-field tc-approval-custom">
                    <span>Custom answer</span>
                    <input
                      className="tc-approval-custom__input"
                      data-testid={`approval-custom-${question.id}`}
                      disabled={submitting}
                      onChange={(event) => updateCustomText(question.id, event.target.value)}
                      placeholder="Enter a custom answer"
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
            Skip
          </button>
          <button
            className="tc-button tc-button--primary"
            data-testid="approval-continue"
            disabled={!canContinue || submitting}
            onClick={submitAnswers}
            type="button"
          >
            {submitting ? "Submitting…" : "Continue"}
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
