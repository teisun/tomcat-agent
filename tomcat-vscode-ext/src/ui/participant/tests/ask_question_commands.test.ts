import { beforeEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { ParticipantCommands } from "../commands";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      reset(): void;
      setInputBoxHandler(handler: () => string): void;
      setQuickPickHandler(
        handler: (items: Array<{ label: string }>) => { label: string } | undefined,
      ): void;
    };
  }
).__testing;

function createRequest() {
  return {
    questions: [
      {
        id: "q1",
        options: [
          { id: "yes", label: "Yes", recommended: true },
          { id: "no", label: "No", recommended: false },
        ],
        prompt: "Proceed?",
      },
    ],
    requestId: "ask-1",
    responseEvent: "plan.ask_question.response.ask-1",
  };
}

describe("ParticipantCommands", () => {
  beforeEach(() => {
    __testing.reset();
  });

  it("renders chat buttons for an attached turn and resolves direct answers", async () => {
    const chatButtons: Array<{ command: string; title: string }> = [];
    const markdowns: string[] = [];
    const participantCommands = new ParticipantCommands({} as never);
    participantCommands.register({ subscriptions: [] } as never);
    participantCommands.attachTurn("s1", {
      button(payload: { command: string; title: string }) {
        chatButtons.push(payload);
      },
      markdown(value: string) {
        markdowns.push(value);
      },
    } as never);

    const pending = participantCommands.askUser(createRequest(), "s1");

    expect(markdowns[0]).toContain("Tomcat 需要你的确认");
    expect(chatButtons.map((button) => button.title)).toEqual([
      "Yes",
      "No",
      "Skip",
      "Other...",
    ]);

    await vscode.commands.executeCommand("tomcat.answer", {
      kind: "direct",
      optionId: "yes",
      pickedRecommended: true,
      questionId: "q1",
      requestId: "ask-1",
    });

    await expect(pending).resolves.toEqual({
      requestId: "ask-1",
      result: {
        answers: [
          {
            customText: null,
            optionIds: ["yes"],
            pickedRecommended: true,
            questionId: "q1",
            skipped: false,
          },
        ],
        cancelled: false,
      },
    });
  });

  it("falls back to quick pick when there is no active turn", async () => {
    __testing.setQuickPickHandler((items: Array<{ label: string }>) =>
      items.find((item) => item.label === "Other..."),
    );
    __testing.setInputBoxHandler(() => "Ship it");

    const participantCommands = new ParticipantCommands({} as never);

    await expect(participantCommands.askUser(createRequest())).resolves.toEqual({
      requestId: "ask-1",
      result: {
        answers: [
          {
            customText: "Ship it",
            optionIds: ["__custom__"],
            pickedRecommended: false,
            questionId: "q1",
            skipped: false,
          },
        ],
        cancelled: false,
      },
    });
  });
});
