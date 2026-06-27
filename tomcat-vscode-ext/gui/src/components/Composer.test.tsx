import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Composer } from "./Composer";

function renderComposer(planState: "chat" | "planning" | "executing" = "planning") {
  return render(
    <Composer
      availableModels={["gpt-5.4"]}
      canPrompt={false}
      contextLabel="Ctx 42%"
      modeValue="plan"
      modelValue="gpt-5.4"
      thinkingLevelValue="high"
      onAddAttachment={vi.fn()}
      onModeChange={vi.fn()}
      onModelChange={vi.fn()}
      onThinkingLevelChange={vi.fn()}
      onPromptChange={vi.fn()}
      onPromptKeyDown={vi.fn()}
      onSubmit={vi.fn()}
      planState={planState}
      prompt=""
      promptPlaceholder="Message Tomcat (Enter to send, Shift+Enter for newline)"
    />,
  );
}

describe("Composer", () => {
  it("renders plan status in the footer instead of the control bar", () => {
    const { container } = renderComposer();

    expect(screen.getByTestId("composer-plan-status-footer").textContent).toBe("Plan: planning");
    expect(container.querySelector(".tc-composer__bar .tc-composer__plan-status")).toBeNull();
    expect(screen.queryByText("Tomcat is responding...")).toBeNull();
  });

  it("omits the footer status when chat mode is active", () => {
    renderComposer("chat");

    expect(screen.queryByTestId("composer-plan-status-footer")).toBeNull();
  });
});
