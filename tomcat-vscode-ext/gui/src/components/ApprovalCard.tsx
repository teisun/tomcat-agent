import type { WebviewApprovalCard } from "../types";

export function ApprovalCard({
  item,
  onAnswer,
}: {
  item: WebviewApprovalCard;
  onAnswer(
    requestId: string,
    questionId: string,
    optionId: string | null,
    pickedRecommended: boolean,
  ): void;
}) {
  if (item.resolved) {
    return null;
  }

  return (
    <section className="tc-card tc-approval-card" data-testid="approval-card">
      <div className="tc-card__header">
        <h3>Approval Required</h3>
        <span className="tc-chip tc-chip--warning">Pending</span>
      </div>
      {item.request.questions.map((question) => (
        <div className="tc-approval-question" key={question.id}>
          <p>{question.prompt}</p>
          <div className="tc-button-row">
            {question.options.map((option) => (
              <button
                className={
                  option.recommended ? "tc-button tc-button--primary" : "tc-button tc-button--secondary"
                }
                key={option.id}
                onClick={() =>
                  onAnswer(item.request.requestId, question.id, option.id, !!option.recommended)
                }
                type="button"
              >
                {option.label}
                {option.recommended ? " (Recommended)" : ""}
              </button>
            ))}
            <button
              className="tc-button tc-button--ghost"
              onClick={() => onAnswer(item.request.requestId, question.id, null, false)}
              type="button"
            >
              Skip
            </button>
          </div>
        </div>
      ))}
    </section>
  );
}
