import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Composer } from "./Composer";

function renderComposer({
  busy = false,
  canPrompt = false,
  onInterrupt = vi.fn(),
  onSubmit = vi.fn(),
  planState = "planning",
}: {
  busy?: boolean;
  canPrompt?: boolean;
  onInterrupt?: () => void;
  onSubmit?: () => void;
  planState?: "chat" | "planning" | "executing";
} = {}) {
  return render(
    <Composer
      availableModels={["gpt-5.4"]}
      busy={busy}
      canPrompt={canPrompt}
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
      onInterrupt={onInterrupt}
      onSubmit={onSubmit}
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
    renderComposer({ planState: "chat" });

    expect(screen.queryByTestId("composer-plan-status-footer")).toBeNull();
  });

  it("swaps the send button for a stop button while busy", () => {
    const onInterrupt = vi.fn();
    renderComposer({
      busy: true,
      canPrompt: true,
      onInterrupt,
    });

    expect(screen.queryByTestId("send-button")).toBeNull();
    fireEvent.click(screen.getByTestId("stop-button"));
    expect(onInterrupt).toHaveBeenCalledTimes(1);
  });
});
