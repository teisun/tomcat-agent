import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type {
  AskQuestionOutcome,
  AskQuestionResult,
  WebviewApprovalQuestion,
} from "../types";
import { AnswerCard } from "./AnswerCard";

const questions: WebviewApprovalQuestion[] = [
  {
    id: "editor",
    options: [
      { id: "vscode", label: "VS Code", recommended: true },
      { id: "vim", label: "Vim" },
    ],
    prompt: "Which editor?",
  },
  {
    id: "language",
    options: [
      { id: "rust", label: "Rust" },
      { id: "typescript", label: "TypeScript", recommended: true },
    ],
    prompt: "Which language?",
  },
];

function result(outcome: AskQuestionOutcome, answers: AskQuestionResult["answers"] = []): AskQuestionResult {
  return {
    answers,
    cancelled: outcome !== "answered",
    outcome,
  };
}

describe("AnswerCard", () => {
  it("shows every question and option while distinguishing answered and question-level skipped", () => {
    render(
      <AnswerCard
        questions={questions}
        result={result("answered", [
          {
            optionIds: ["vscode"],
            pickedRecommended: true,
            questionId: "editor",
          },
          {
            optionIds: [],
            pickedRecommended: false,
            questionId: "language",
            skipped: true,
          },
        ])}
      />,
    );

    expect(screen.getAllByTestId("answer-card-question")).toHaveLength(2);
    expect(screen.getByTestId("answer-card-outcome").textContent).toBe("1 answered · 1 skipped");
    expect(screen.getByTestId("answer-status-editor").textContent).toBe("Answered");
    expect(screen.getByTestId("answer-status-language").textContent).toBe("Skipped");
    expect(screen.getByTestId("answer-option-editor").getAttribute("data-option-id")).toBe("vscode");

    for (const prompt of ["Which editor?", "Which language?"]) {
      const optionList = screen.getByRole("list", { name: new RegExp(prompt) });
      expect(within(optionList).getAllByRole("listitem")).toHaveLength(3);
    }
  });

  for (const [outcome, label] of [
    ["skipped", "Entire request skipped"],
    ["interrupted", "Interrupted"],
    ["host_disconnected", "Disconnected"],
    ["cancelled_unknown", "Cancelled"],
  ] as const) {
    it(`renders ${outcome} explicitly without hiding questions or options`, () => {
      render(<AnswerCard questions={questions} result={result(outcome)} />);

      expect(screen.getByTestId("answer-card").getAttribute("data-outcome")).toBe(outcome);
      expect(screen.getByTestId("answer-card-outcome").textContent).toBe(label);
      expect(screen.getAllByTestId("answer-card-question")).toHaveLength(2);
      expect(screen.getAllByRole("listitem")).toHaveLength(6);
      expect(screen.queryByTestId("answer-status-editor")).toBeNull();
      expect(screen.queryByTestId("answer-status-language")).toBeNull();
    });
  }

  it("labels an old cancelled payload without exposing its compatibility detail", () => {
    render(
      <AnswerCard
        questions={questions}
        result={{ answers: [], cancelled: true }}
      />,
    );

    expect(screen.getByTestId("answer-card").getAttribute("data-outcome")).toBe(
      "cancelled_unknown",
    );
    expect(screen.getByTestId("answer-card-outcome").textContent).toBe(
      "Cancelled",
    );
  });
});
