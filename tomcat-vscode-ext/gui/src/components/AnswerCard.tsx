import {
  CUSTOM_OPTION_ID,
  type AskQuestionAnswer,
  type AskQuestionResult,
  type WebviewApprovalOption,
  type WebviewApprovalQuestion,
} from "../types";

function buildOptionCode(index: number): string {
  let remaining = index;
  let code = "";

  do {
    code = String.fromCharCode(65 + (remaining % 26)) + code;
    remaining = Math.floor(remaining / 26) - 1;
  } while (remaining >= 0);

  return code;
}

function selectedOptionIndex(
  question: WebviewApprovalQuestion,
  answer: AskQuestionAnswer,
): number {
  if (!answer.optionIds.length || answer.optionIds[0] === CUSTOM_OPTION_ID) {
    return question.options.length;
  }
  const optionIndex = question.options.findIndex((option) => option.id === answer.optionIds[0]);
  return optionIndex >= 0 ? optionIndex : question.options.length;
}

function resolveSelectedOption(
  question: WebviewApprovalQuestion,
  answer: AskQuestionAnswer,
): WebviewApprovalOption | null {
  if (!answer.optionIds.length || answer.optionIds[0] === CUSTOM_OPTION_ID) {
    return null;
  }
  return question.options.find((option) => option.id === answer.optionIds[0]) ?? null;
}

function answerLabel(question: WebviewApprovalQuestion, answer: AskQuestionAnswer): string {
  if (answer.skipped || !answer.optionIds.length) {
    return "Skipped";
  }
  if (answer.optionIds[0] === CUSTOM_OPTION_ID) {
    return answer.customText?.trim() || "Other";
  }
  return resolveSelectedOption(question, answer)?.label ?? answer.optionIds[0];
}

export function AnswerCard({
  questions,
  result,
}: {
  questions: WebviewApprovalQuestion[];
  result: AskQuestionResult;
}) {
  const answersByQuestion = new Map(result.answers.map((answer) => [answer.questionId, answer]));
  const answeredCount = result.cancelled ? 0 : result.answers.length;

  return (
    <section className="tc-card tc-answer-card" data-testid="answer-card">
      <div className="tc-card__header">
        <h3>Answers</h3>
        <span className="tc-chip">{`${answeredCount} answered`}</span>
      </div>
      <div className="tc-answer-card__questions">
        {questions.map((question, questionIndex) => {
          const answer = answersByQuestion.get(question.id);
          if (!answer) {
            return null;
          }
          const selectedOption = resolveSelectedOption(question, answer);
          const selectedLabel = answerLabel(question, answer);
          const optionCode = buildOptionCode(selectedOptionIndex(question, answer));

          return (
            <div className="tc-approval-question" data-testid="answer-card-question" key={question.id}>
              <div className="tc-approval-question__prompt">
                <span className="tc-approval-question__index">{questionIndex + 1}.</span>
                <p>{question.prompt}</p>
              </div>
              <div
                className="tc-approval-option tc-approval-option--selected tc-answer-card__option"
                data-testid={`answer-option-${question.id}`}
              >
                <span
                  aria-hidden="true"
                  className="tc-approval-option__code tc-approval-option__code--selected"
                >
                  {optionCode}
                </span>
                <span className="tc-approval-option__content">
                  <span className="tc-approval-option__label">{selectedLabel}</span>
                  {selectedOption?.recommended ? (
                    <span className="tc-approval-option__recommended">Recommended</span>
                  ) : null}
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
