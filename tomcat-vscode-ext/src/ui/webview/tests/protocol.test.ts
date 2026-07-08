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
        channel: "event",
        content: {
          reference: {
            kind: "selection",
            label: "app.ts:1-2",
            lineEnd: 2,
            lineStart: 1,
            path: "app.ts",
            text: "const x = 1;",
            type: "reference",
          },
          sessionId: "s1",
          type: "insertReference",
        },
        messageId: "event-1",
      }),
    ).toBe(true);

    expect(
      isWebviewIntent({
        data: {
          sessionId: "s1",
        },
        messageId: "pick-1",
        type: "pickContext",
      }),
    ).toBe(true);

    expect(
      isWebviewIntent({
        data: {
          segments: [
            { text: "Inspect ", type: "text" },
            {
              kind: "file",
              label: "app.ts",
              path: "app.ts",
              type: "reference",
            },
          ],
          text: "Inspect app.ts",
        },
        messageId: "prompt-1",
        type: "prompt",
      }),
    ).toBe(true);

    expect(
      isWebviewIntent({
        data: {
          sessionId: "s1",
          uris: ["file:///workspace/app.ts"],
        },
        messageId: "drop-1",
        type: "resolveDrop",
      }),
    ).toBe(true);

    expect(
      isWebviewIntent({
        data: {
          route: "models",
        },
        messageId: "settings-1",
        type: "openModelSettings",
      }),
    ).toBe(true);
  });

  it("rejects malformed intents", () => {
    expect(
      isWebviewIntent({
        data: {},
        messageId: "legacy-pick-1",
        type: "pickAttachment",
      }),
    ).toBe(false);
    expect(
      isWebviewIntent({
        data: {},
        messageId: "prompt-1",
        type: "prompt",
      }),
    ).toBe(false);
    expect(
      isWebviewIntent({
        data: {
          segments: [
            {
              kind: "file",
              label: 123,
              path: "app.ts",
              type: "reference",
            },
          ],
          text: "bad",
        },
        messageId: "prompt-2",
        type: "prompt",
      }),
    ).toBe(false);
    expect(
      isWebviewIntent({
        data: {
          uris: ["file:///workspace/app.ts", 42],
        },
        messageId: "drop-2",
        type: "resolveDrop",
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
