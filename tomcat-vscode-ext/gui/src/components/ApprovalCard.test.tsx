import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  AskQuestionResult,
  WebviewApprovalCard,
  WebviewApprovalQuestion,
} from "../types";
import {
  ApprovalCard,
  createApprovalAnswerDraft,
  type ApprovalAnswerDraft,
} from "./ApprovalCard";

function buildQuestion(
  id: string,
  prompt: string,
  options = [
    { id: `${id}-a`, label: "Option A", recommended: true },
    { id: `${id}-b`, label: "Option B" },
  ],
): WebviewApprovalQuestion {
  return {
    id,
    options,
    prompt,
  };
}

function buildItem(
  questions: WebviewApprovalQuestion[],
  overrides: Partial<WebviewApprovalCard> = {},
): WebviewApprovalCard {
  return {
    id: "approval-1",
    live: true,
    request: {
      questions,
      requestId: "request-1",
      responseEvent: "response",
    },
    resolved: false,
    sessionId: "session-1",
    type: "approval",
    ...overrides,
  };
}

function ControlledApprovalCard({
  item,
  onAnswer,
  pendingGroupCount = 1,
  presentation = "default",
}: {
  item: WebviewApprovalCard;
  onAnswer: (sessionId: string, requestId: string, result: AskQuestionResult) => void;
  pendingGroupCount?: number;
  presentation?: "default" | "pending";
}) {
  const [collapsed, setCollapsed] = useState(false);
  const [draft, setDraft] = useState<ApprovalAnswerDraft>(() => createApprovalAnswerDraft(item));
  return (
    <ApprovalCard
      collapsed={collapsed}
      draft={draft}
      item={item}
      onAnswer={onAnswer}
      onCollapsedChange={setCollapsed}
      onDraftChange={(_sessionId, _requestId, next) => setDraft(next)}
      pendingGroupCount={pendingGroupCount}
      presentation={presentation}
    />
  );
}

describe("ApprovalCard", () => {
  it("renders numbered questions, coded options, recommended badge, and action buttons", () => {
    render(
      <ControlledApprovalCard
        item={buildItem([
          buildQuestion("q1", "When do you prefer to code?"),
          buildQuestion("q2", "Which language do you want to use?"),
        ])}
        onAnswer={vi.fn()}
      />,
    );

    expect(screen.getByText("Questions")).toBeTruthy();
    expect(screen.getByText("2 of 2")).toBeTruthy();
    expect(screen.getByText("2 of 2").className).toBe("tc-chip tc-chip--warning");
    expect(screen.getByRole("heading", { name: "Questions" }).querySelector(".codicon-question")).toBeNull();
    expect(screen.getByText("1.")).toBeTruthy();
    expect(screen.getByText("2.")).toBeTruthy();
    expect(screen.getAllByText("A")).toHaveLength(2);
    expect(screen.getAllByText("B")).toHaveLength(2);
    expect(screen.getAllByText("Other...")).toHaveLength(2);
    expect(screen.getAllByText("Recommended")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Skip" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
  });

  it("uses the active question count, navigation, and an icon-only collapsible pending dock", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard
        item={buildItem([
          buildQuestion("q1", "Pick a time"),
          buildQuestion("q2", "Pick a language"),
        ])}
        onAnswer={onAnswer}
        pendingGroupCount={2}
        presentation="pending"
      />,
    );

    expect(screen.getByTestId("approval-question-count").textContent).toBe("1 of 2");
    expect(screen.getByTestId("approval-question-count").className).toBe("tc-approval-card__count");
    expect(screen.getAllByRole("radio", { name: "Option A Recommended" })).toHaveLength(1);
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
    expect(screen.queryByText("Pick a language")).toBeNull();
    fireEvent.click(screen.getByTestId("approval-option-q1-q1-a"));
    expect(screen.getByRole("heading", { name: "Questions" }).querySelector(".codicon-question")).toBeTruthy();
    expect(screen.getByTestId("approval-next-question").getAttribute("title")).toBe("Next question");
    expect((screen.getByTestId("approval-previous-question") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId("approval-next-question") as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText("1 more question group waiting")).toBeTruthy();
    expect(screen.getByTestId("approval-collapse").getAttribute("title")).toBe("Collapse questions");
    expect(screen.getByTestId("approval-collapse").querySelector(".codicon-chevron-down")).toBeTruthy();

    fireEvent.click(screen.getByTestId("approval-next-question"));
    expect(screen.getByTestId("approval-question-count").textContent).toBe("2 of 2");
    expect((screen.getByTestId("approval-previous-question") as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId("approval-next-question") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
    expect(screen.queryByText("Pick a time")).toBeNull();
    expect(screen.getByText("Pick a language")).toBeTruthy();
    fireEvent.scroll(screen.getByTestId("approval-questions-body"), { target: { scrollTop: 100 } });
    fireEvent(window, new Event("resize"));
    expect(screen.getByTestId("approval-question-count").textContent).toBe("2 of 2");
    expect(onAnswer).not.toHaveBeenCalled();

    const secondAnswer = screen.getByTestId("approval-option-q2-q2-a");
    fireEvent.click(secondAnswer);
    secondAnswer.focus();
    fireEvent.click(screen.getByTestId("approval-collapse"));
    expect(document.activeElement).toBe(screen.getByTestId("approval-collapse"));
    expect(screen.queryByTestId("approval-questions-body")).toBeNull();
    expect(screen.queryByTestId("approval-continue")).toBeNull();
    expect(screen.getByTestId("approval-collapse").getAttribute("aria-label")).toBe("Expand questions");
    expect(screen.getByTestId("approval-collapse").querySelector(".codicon-chevron-right")).toBeTruthy();

    fireEvent.click(screen.getByTestId("approval-collapse"));
    expect(screen.getByTestId("approval-question-count").textContent).toBe("2 of 2");
    expect(screen.getByTestId("approval-option-q2-q2-a").getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByTestId("approval-option-q1-q1-a")).toBeNull();
    fireEvent.click(screen.getByTestId("approval-previous-question"));
    expect(screen.getByTestId("approval-questions-body").scrollTop).toBe(0);
    expect(screen.getByTestId("approval-option-q1-q1-a").getAttribute("aria-checked")).toBe("true");
  });

  it("retains Other text across pages and submits the full ordered batch once", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard
        item={buildItem([buildQuestion("q1", "First?"), buildQuestion("q2", "Second?")])}
        onAnswer={onAnswer}
        presentation="pending"
      />,
    );
    fireEvent.click(screen.getByTestId("approval-option-q1-__custom__"));
    expect(document.activeElement).toBe(screen.getByTestId("approval-custom-q1"));
    fireEvent.change(screen.getByTestId("approval-custom-q1"), { target: { value: "  My answer  " } });
    expect((screen.getByTestId("approval-continue") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByTestId("approval-next-question"));
    expect(screen.queryByTestId("approval-custom-q1")).toBeNull();
    fireEvent.click(screen.getByTestId("approval-option-q2-q2-a"));
    fireEvent.click(screen.getByTestId("approval-previous-question"));
    expect((screen.getByTestId("approval-custom-q1") as HTMLInputElement).value).toBe("  My answer  ");
    fireEvent.click(screen.getByTestId("approval-continue"));
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(onAnswer).toHaveBeenCalledWith("session-1", "request-1", {
      answers: [
        { questionId: "q1", optionIds: ["__custom__"], customText: "My answer", pickedRecommended: false },
        { questionId: "q2", optionIds: ["q2-a"], pickedRecommended: true },
      ],
      cancelled: false,
      outcome: "answered",
    });
  });

  it("starts a replacement request on its first page", () => {
    const onAnswer = vi.fn();
    const { rerender } = render(
      <ControlledApprovalCard
        item={buildItem([buildQuestion("q1", "First?"), buildQuestion("q2", "Second?")])}
        onAnswer={onAnswer}
        presentation="pending"
      />,
    );
    fireEvent.click(screen.getByTestId("approval-next-question"));
    rerender(
      <ControlledApprovalCard
        item={buildItem([buildQuestion("new", "New question?")], {
          request: { requestId: "request-2", responseEvent: "response-2", questions: [buildQuestion("new", "New question?")] },
        })}
        onAnswer={onAnswer}
        presentation="pending"
      />,
    );
    expect(screen.getByTestId("approval-question-count").textContent).toBe("1 of 1");
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
    expect(screen.getByText("New question?")).toBeTruthy();
  });

  it("does not render resolved cards", () => {
    const { container } = render(
      <ControlledApprovalCard
        item={buildItem([buildQuestion("q1", "Proceed?")], { resolved: true })}
        onAnswer={vi.fn()}
      />,
    );

    expect(container.innerHTML).toBe("");
  });

  it("renders a passive restoring card without answer controls for history-only approvals", () => {
    render(
      <ControlledApprovalCard
        item={buildItem([buildQuestion("q1", "Proceed?")], { live: false })}
        onAnswer={vi.fn()}
      />,
    );

    expect(screen.getByTestId("approval-card-restoring")).toBeTruthy();
    expect(screen.queryByTestId("approval-continue")).toBeNull();
    expect(screen.queryByTestId("approval-skip")).toBeNull();
  });

  it("keeps Continue disabled until every question is answered", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard
        item={buildItem([
          buildQuestion("q1", "Pick a time"),
          buildQuestion("q2", "Pick a language"),
        ])}
        onAnswer={onAnswer}
      />,
    );

    const continueButton = screen.getByTestId("approval-continue");
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(continueButton);
    expect(onAnswer).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId("approval-option-q1-q1-a"));
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId("approval-option-q2-q2-b"));
    expect((continueButton as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps radio selection exclusive within a question and independent across questions", () => {
    render(
      <ControlledApprovalCard
        item={buildItem([
          buildQuestion("q1", "Pick a time"),
          buildQuestion("q2", "Pick a language"),
        ])}
        onAnswer={vi.fn()}
      />,
    );

    const q1OptionA = screen.getByTestId("approval-option-q1-q1-a");
    const q1OptionB = screen.getByTestId("approval-option-q1-q1-b");
    const q2OptionA = screen.getByTestId("approval-option-q2-q2-a");

    fireEvent.click(q1OptionA);
    expect(q1OptionA.getAttribute("aria-checked")).toBe("true");
    expect(q1OptionB.getAttribute("aria-checked")).toBe("false");
    expect(q2OptionA.getAttribute("aria-checked")).toBe("false");

    fireEvent.click(q1OptionB);
    expect(q1OptionA.getAttribute("aria-checked")).toBe("false");
    expect(q1OptionB.getAttribute("aria-checked")).toBe("true");
    expect(q2OptionA.getAttribute("aria-checked")).toBe("false");
  });

  it("submits ordered batch answers with pickedRecommended flags", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard
        item={buildItem([
          buildQuestion("q1", "Pick a time"),
          buildQuestion("q2", "Pick a language"),
        ])}
        onAnswer={onAnswer}
      />,
    );

    fireEvent.click(screen.getByTestId("approval-option-q1-q1-a"));
    fireEvent.click(screen.getByTestId("approval-option-q2-q2-b"));
    fireEvent.click(screen.getByTestId("approval-continue"));

    expect(onAnswer).toHaveBeenCalledWith("session-1", "request-1", {
      answers: [
        {
          optionIds: ["q1-a"],
          pickedRecommended: true,
          questionId: "q1",
        },
        {
          optionIds: ["q2-b"],
          pickedRecommended: false,
          questionId: "q2",
        },
      ],
      cancelled: false,
      outcome: "answered",
    });
  });

  it("requires non-empty custom text for Other and trims it on submit", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard item={buildItem([buildQuestion("q1", "Pick a time")])} onAnswer={onAnswer} />,
    );

    fireEvent.click(screen.getByTestId("approval-option-q1-__custom__"));

    const continueButton = screen.getByTestId("approval-continue");
    const customInput = screen.getByTestId("approval-custom-q1");

    expect(customInput).toBeTruthy();
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(customInput, { target: { value: "   " } });
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(customInput, { target: { value: "  Svelte  " } });
    expect((continueButton as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(continueButton);
    expect(onAnswer).toHaveBeenCalledWith("session-1", "request-1", {
      answers: [
        {
          customText: "Svelte",
          optionIds: ["__custom__"],
          pickedRecommended: false,
          questionId: "q1",
        },
      ],
      cancelled: false,
      outcome: "answered",
    });
  });

  it("submits a cancelled result when Skip is clicked", () => {
    const onAnswer = vi.fn();
    render(
      <ControlledApprovalCard item={buildItem([buildQuestion("q1", "Pick a time")])} onAnswer={onAnswer} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Skip" }));

    expect(onAnswer).toHaveBeenCalledWith("session-1", "request-1", {
      answers: [],
      cancelled: true,
      outcome: "skipped",
    });
  });

  it("isolates local selection state between multiple cards", () => {
    render(
      <>
        <ControlledApprovalCard
          item={buildItem([buildQuestion("q1", "Pick a time")], {
            id: "approval-1",
            request: {
              questions: [buildQuestion("q1", "Pick a time")],
              requestId: "request-1",
              responseEvent: "response-1",
            },
          })}
          onAnswer={vi.fn()}
        />
        <ControlledApprovalCard
          item={buildItem([buildQuestion("q2", "Pick a language")], {
            id: "approval-2",
            request: {
              questions: [buildQuestion("q2", "Pick a language")],
              requestId: "request-2",
              responseEvent: "response-2",
            },
          })}
          onAnswer={vi.fn()}
        />
      </>,
    );

    const continueButtons = screen.getAllByTestId("approval-continue");
    fireEvent.click(screen.getByTestId("approval-option-q1-q1-a"));

    expect((continueButtons[0] as HTMLButtonElement).disabled).toBe(false);
    expect((continueButtons[1] as HTMLButtonElement).disabled).toBe(true);
  });
});
