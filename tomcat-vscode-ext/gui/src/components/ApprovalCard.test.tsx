import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewApprovalCard, WebviewApprovalQuestion } from "../types";
import { ApprovalCard } from "./ApprovalCard";

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

describe("ApprovalCard", () => {
  it("renders numbered questions, coded options, recommended badge, and action buttons", () => {
    render(
      <ApprovalCard
        item={buildItem([
          buildQuestion("q1", "When do you prefer to code?"),
          buildQuestion("q2", "Which language do you want to use?"),
        ])}
        onAnswer={vi.fn()}
      />,
    );

    expect(screen.getByText("Questions")).toBeTruthy();
    expect(screen.getByText("2 of 2")).toBeTruthy();
    expect(screen.getByText("1.")).toBeTruthy();
    expect(screen.getByText("2.")).toBeTruthy();
    expect(screen.getAllByText("A")).toHaveLength(2);
    expect(screen.getAllByText("B")).toHaveLength(2);
    expect(screen.getAllByText("Other...")).toHaveLength(2);
    expect(screen.getAllByText("Recommended")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Skip" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
  });

  it("does not render resolved cards", () => {
    const { container } = render(
      <ApprovalCard
        item={buildItem([buildQuestion("q1", "Proceed?")], { resolved: true })}
        onAnswer={vi.fn()}
      />,
    );

    expect(container.innerHTML).toBe("");
  });

  it("keeps Continue disabled until every question is answered", () => {
    const onAnswer = vi.fn();
    render(
      <ApprovalCard
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
      <ApprovalCard
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
      <ApprovalCard
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

    expect(onAnswer).toHaveBeenCalledWith("request-1", {
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
    });
  });

  it("requires non-empty custom text for Other and trims it on submit", () => {
    const onAnswer = vi.fn();
    render(
      <ApprovalCard item={buildItem([buildQuestion("q1", "Pick a time")])} onAnswer={onAnswer} />,
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
    expect(onAnswer).toHaveBeenCalledWith("request-1", {
      answers: [
        {
          customText: "Svelte",
          optionIds: ["__custom__"],
          pickedRecommended: false,
          questionId: "q1",
        },
      ],
      cancelled: false,
    });
  });

  it("submits a cancelled result when Skip is clicked", () => {
    const onAnswer = vi.fn();
    render(
      <ApprovalCard item={buildItem([buildQuestion("q1", "Pick a time")])} onAnswer={onAnswer} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Skip" }));

    expect(onAnswer).toHaveBeenCalledWith("request-1", {
      answers: [],
      cancelled: true,
    });
  });

  it("isolates local selection state between multiple cards", () => {
    render(
      <>
        <ApprovalCard
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
        <ApprovalCard
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
