import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MessageBubble } from "./MessageBubble";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";

describe("MessageBubble", () => {
  it.each(["Single line", "First line\nSecond line"])("keeps the file-revert edit affordance clickable for %j", (text) => {
    const onEdit = vi.fn();
    const item = {id:"editable",kind:"user" as const,type:"message" as const,text,rewindEligible:true};
    render(<MessageBubble item={item} onEdit={onEdit} hasFileWrites />);
    const button = screen.getByRole("button", {name:"Edit message"});
    expect(button.className).toContain("tc-message__edit-trigger--revert");
    expect(button.querySelector(".codicon-discard")).not.toBeNull();
    expect(screen.getByTestId("message-block").getAttribute("data-has-writes")).toBe("true");
    fireEvent.click(button);
    expect(onEdit).toHaveBeenCalledExactlyOnceWith(item);
    expect(screen.getByTestId("message-text").textContent).toBe(text);
  });

  it("retains a focusable edit entry without reserving revert-icon space", () => {
    const onEdit = vi.fn();
    const item = { id: "no-write", kind: "user" as const, type: "message" as const, text: "No writes", rewindEligible: true };
    render(<MessageBubble item={item} onEdit={onEdit} />);
    expect(screen.getByTestId("message-block").getAttribute("data-has-writes")).toBe("false");
    const button = screen.getByRole("button", { name: "Edit message" });
    expect(button.tabIndex).toBe(0);
    button.focus();
    expect(document.activeElement).toBe(button);
    fireEvent.click(screen.getByTestId("message-text"));
    expect(onEdit).toHaveBeenCalledExactlyOnceWith(item);
  });

  it("renders user message as right pill without header", () => {
    render(
      <MessageBubble
        item={{
          id: "u1",
          kind: "user",
          text: "Hello Tomcat",
          type: "message",
        }}
      />,
    );

    const node = screen.getByTestId("message-block");
    expect(node.className).toContain("tc-message--user");
    expect(screen.queryByText("You")).toBeNull();
    expect(screen.getByTestId("message-text").textContent).toContain("Hello Tomcat");
  });

  it("renders assistant markdown without the old assistant header", () => {
    render(
      <MessageBubble
        item={{
          id: "a1",
          kind: "assistant",
          text: "Here is **the** answer.",
          type: "message",
        }}
      />,
    );

    const node = screen.getByTestId("message-block");
    expect(node.className).toContain("tc-message--assistant");
    expect(screen.queryByText("Tomcat")).toBeNull();
    expect(screen.queryByText("assistant")).toBeNull();
    expect(screen.getByTestId("message-text").querySelector("strong")?.textContent).toBe("the");
  });

  it("renders assistant code fences as clickable code cards", async () => {
    const onOpenFile = vi.fn();
    render(
      <MessageBubble
        item={{
          id: "a-code",
          kind: "assistant",
          text: "```rust src/lib.rs:9\nfn answer() -> i32 { 42 }\n```",
          type: "message",
        }}
        onOpenFile={onOpenFile}
      />,
    );

    await screen.findByTestId("assistant-code-card");
    fireEvent.click(screen.getByTestId("assistant-code-file"));
    expect(onOpenFile).toHaveBeenCalledWith("src/lib.rs", 9);
  });

  it("forwards assistant Markdown links to the transcript link handler", () => {
    const onOpenLink = vi.fn();
    render(
      <MessageBubble
        item={{
          id: "a-link",
          kind: "assistant",
          text: "See [the docs](https://example.com/docs).",
          type: "message",
        }}
        onOpenLink={onOpenLink}
      />,
    );

    fireEvent.click(screen.getByRole("link", { name: "the docs" }));
    expect(onOpenLink).toHaveBeenCalledWith("https://example.com/docs");
  });

  it("keeps left border for error and notice", () => {
    const { rerender } = render(
      <MessageBubble
        item={{
          id: "e1",
          kind: "error",
          text: "boom",
          type: "message",
        }}
      />,
    );
    expect(screen.getByTestId("message-block").className).toContain("tc-message--error");

    rerender(
      <MessageBubble
        item={{
          id: "n1",
          kind: "notice",
          text: "note",
          type: "message",
        }}
      />,
    );
    expect(screen.getByTestId("message-block").className).toContain("tc-message--notice");

    rerender(
      <MessageBubble
        item={{
          id: "w1",
          kind: "warn",
          text: "careful",
          type: "message",
        }}
      />,
    );
    expect(screen.getByTestId("message-block").className).toContain("tc-message--warn");
    expect(screen.getByText("Warn")).toBeTruthy();
  });

  it("renders failed user status with retry", () => {
    let retriedMessageId: string | null = null;
    render(
      <MessageBubble
        item={{
          deliveryError: "Session is busy",
          deliveryState: "failed",
          id: "u-failed",
          kind: "user",
          retryable: true,
          submitKind: "prompt",
          text: "Retry me",
          type: "message",
        }}
        onRetry={(messageId) => {
          retriedMessageId = messageId;
        }}
      />,
    );

    expect(screen.getByTestId("user-message-status").textContent).toContain("Session is busy");
    fireEvent.click(screen.getByTestId("retry-user-message"));
    expect(retriedMessageId).toBe("u-failed");
  });

  it.each(["en", "zh-CN"] as const)("renders a superseded failed user message as an abandoned bubble (%s)", (locale) => {
    render(
      <LocaleProvider locale={locale}><MessageBubble
        item={{
          abandoned: true,
          id: "u-abandoned",
          kind: "user",
          text: "Retry me",
          type: "message",
        }}
      /></LocaleProvider>,
    );

    expect(screen.getByTestId("message-block").className).toContain("tc-message--user-abandoned");
    const status = screen.getByTestId("abandoned-user-message-status");
    expect(status.isConnected).toBe(true);
    if (locale === "en") expect(status.textContent).toContain(translate("en", "message.abandoned"));
  });

  it("shows a rejected recovery reason on the persistent error card", () => {
    render(
      <MessageBubble
        item={{
          id: "error-rejected",
          kind: "error",
          recoveryError: "This error card is stale. Refresh the session and try again.",
          text: "Request failed",
          type: "message",
        }}
      />,
    );

    expect(screen.getByTestId("error-recovery-rejection").textContent).toContain(
      "This error card is stale",
    );
  });

  it("separates Retry or Resume as an icon-backed primary action", () => {
    const onRecover = vi.fn();
    const { rerender } = render(
      <MessageBubble
        item={{
          detailText: "raw provider payload",
          id: "error-1",
          kind: "error",
          recoveryAction: "retry",
          text: "Request failed",
          type: "message",
        }}
        onRecover={onRecover}
      />,
    );
    fireEvent.click(screen.getByTestId("recover-error-turn"));
    expect(onRecover).toHaveBeenCalledWith("error-1", "retry");
    expect(screen.getByTestId("error-detail-actions").textContent).not.toContain("Retry");
    expect(
      screen
        .getByTestId("error-recovery-actions")
        .contains(screen.getByTestId("recover-error-turn")),
    ).toBe(true);
    expect(screen.getByTestId("recover-error-turn").className).toContain("tc-button--primary");
    expect(
      screen
        .getByTestId("recover-error-turn")
        .querySelector(".codicon-refresh"),
    ).not.toBeNull();

    rerender(
      <MessageBubble
        item={{
          id: "error-2",
          kind: "error",
          recoveryAction: "resume",
          text: "Stream interrupted",
          type: "message",
        }}
        onRecover={onRecover}
      />,
    );
    expect(screen.getByTestId("recover-error-turn").textContent).toBe("Resume");
    expect(
      screen
        .getByTestId("recover-error-turn")
        .querySelector(".codicon-debug-continue"),
    ).not.toBeNull();
  });

  it("renders pending user status", () => {
    render(
      <MessageBubble
        item={{
          deliveryState: "pending",
          id: "u-pending",
          kind: "user",
          submitKind: "steer",
          text: "Hold on",
          type: "message",
        }}
      />,
    );

    expect(screen.getByTestId("user-message-status").textContent).toContain("Sending...");
    expect(screen.queryByTestId("retry-user-message")).toBeNull();
  });

  it("renders interleaved history reference chips with hover titles", () => {
    render(
      <MessageBubble
        item={{
          id: "u-ref",
          kind: "user",
          segments: [
            { text: "Please inspect ", type: "text" },
            {
              kind: "selection",
              label: "app.ts:3-5",
              lineEnd: 5,
              lineStart: 3,
              path: "src/app.ts",
              text: "const answer = 42;",
              type: "reference",
            },
            { text: " before editing.", type: "text" },
          ],
          text: "Please inspect app.ts:3-5 before editing.",
          type: "message",
        }}
      />,
    );

    expect(screen.getByTestId("message-text").textContent).toContain(
      "Please inspect app.ts:3-5 before editing.",
    );
    expect(screen.getByTestId("history-reference-chip").getAttribute("title")).toBe(
      "src/app.ts:3-5",
    );
  });

  it("shows and copies the original raw error detail on demand", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(globalThis.navigator, {
      clipboard: {
        writeText,
      },
    });

    render(
      <MessageBubble
        item={{
          detailText: "API 错误 403: <html>forbidden</html>",
          id: "err-detail",
          kind: "error",
          text: "API 错误 403 · aigateway.sunmi.com · Request-Id req-1",
          type: "message",
        }}
      />,
    );

    expect(screen.queryByTestId("error-detail-text")).toBeNull();
    fireEvent.click(screen.getByTestId("toggle-error-detail"));
    expect(screen.getByTestId("error-detail-text").textContent).toContain("forbidden");
    await act(async () => {
      fireEvent.click(screen.getByTestId("copy-error-detail"));
    });
    expect(writeText).toHaveBeenCalledWith("API 错误 403: <html>forbidden</html>");
  });
});
