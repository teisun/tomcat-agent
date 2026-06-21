import { describe, expect, it } from "vitest";

import {
  isHostToWebviewFrame,
  isWebviewIntent,
  PendingMessageTracker,
} from "../protocol";

describe("webview protocol helpers", () => {
  it("accepts valid host and webview frames", () => {
    expect(
      isHostToWebviewFrame({
        channel: "state",
        content: {
          activeSessionId: "s1",
          availableModels: [],
          ready: true,
          sessionViews: {},
          sessions: [],
          uiMode: "both",
        },
        messageId: "state-1",
      }),
    ).toBe(true);

    expect(
      isWebviewIntent({
        data: { text: "hello" },
        messageId: "prompt-1",
        type: "prompt",
      }),
    ).toBe(true);
  });

  it("rejects malformed intents", () => {
    expect(
      isWebviewIntent({
        data: {},
        messageId: "prompt-1",
        type: "prompt",
      }),
    ).toBe(false);
    expect(
      isHostToWebviewFrame({
        channel: "state",
        content: null,
        messageId: "state-1",
      }),
    ).toBe(false);
  });

  it("drops unknown pending message ids", async () => {
    const tracker = new PendingMessageTracker<string>();
    const pending = tracker.create("known", 1_000);

    expect(tracker.resolve("unknown", "ignored")).toBe(false);
    expect(tracker.resolve("known", "done")).toBe(true);
    await expect(pending).resolves.toBe("done");
  });
});
