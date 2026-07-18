import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MessageBubble } from "./MessageBubble";

describe("MessageBubble", () => {
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

  it("renders assistant message without card or header", () => {
    render(
      <MessageBubble
        item={{
          id: "a1",
          kind: "assistant",
          text: "Here is the answer.",
          type: "message",
        }}
      />,
    );

    const node = screen.getByTestId("message-block");
    expect(node.className).toContain("tc-message--assistant");
    expect(screen.queryByText("Tomcat")).toBeNull();
    expect(screen.queryByText("assistant")).toBeNull();
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
