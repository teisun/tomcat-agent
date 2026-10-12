import { describe, expect, it } from "vitest";

import { isWebviewIntent } from "../protocol";

describe("setContextWindow webview intent", () => {
  it("accepts a positive integer tier with the current model/session shape", () => {
    expect(
      isWebviewIntent({
        data: {
          contextWindow: 1_000_000,
          modelId: "gpt-5.6",
          sessionId: "s1",
        },
        messageId: "context-tier",
        type: "setContextWindow",
      }),
    ).toBe(true);
  });

  it("rejects non-integer and non-positive tiers", () => {
    for (const contextWindow of [0, -1, 400_000.5, "400000"]) {
      expect(
        isWebviewIntent({
          data: { contextWindow, modelId: "gpt-5.6" },
          messageId: "bad-context-tier",
          type: "setContextWindow",
        }),
      ).toBe(false);
    }
  });
});
