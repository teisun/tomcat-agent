import { describe, expect, it } from "vitest";

import { WebviewStateStore } from "../src/ui/webview/state";

describe("backpressure notice integration", () => {
  it("surfaces llm_notice in the webview timeline while preserving streamed content", () => {
    const store = new WebviewStateStore("webview");

    for (const delta of ["a", "b", "c", "d"]) {
      store.applyEvent({
        assistantMessageId: "assistant-1",
        assistantMessageEvent: { delta, kind: "content_delta" },
        message: {},
        sessionId: "s1",
        type: "message_update",
      });
    }

    store.applyEvent({
      finishReason: "backpressure",
      message: "Tomcat dropped intermediate deltas to keep up.",
      sessionId: "s1",
      type: "llm_notice",
    });

    store.applyEvent({
      error: null,
      messages: [],
      sessionId: "s1",
      type: "agent_end",
    });

    const timeline = store.snapshot().sessionViews.s1.timeline;

    const assistantText = timeline
      .filter((item) => item.type === "message" && item.kind === "assistant")
      .map((item) => (item as { text: string }).text)
      .join("");
    expect(assistantText).toBe("abcd");

    const noticeIndex = timeline.findIndex(
      (item) =>
        item.type === "message" &&
        item.kind === "notice" &&
        (item as { text: string }).text ===
          "Tomcat dropped intermediate deltas to keep up.",
    );
    const assistantIndex = timeline.findIndex(
      (item) => item.type === "message" && item.kind === "assistant",
    );

    expect(noticeIndex).toBeGreaterThanOrEqual(0);
    expect(noticeIndex).toBeGreaterThan(assistantIndex);
  });
});
