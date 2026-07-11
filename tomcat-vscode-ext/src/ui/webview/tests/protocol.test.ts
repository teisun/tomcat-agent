import { describe, expect, it } from "vitest";

import {
  coerceContextSearchResultEvent,
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
          kind: "file",
          query: "app",
          requestId: "req-1",
          sessionId: "s1",
        },
        messageId: "search-1",
        type: "searchContext",
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
        messageId: "search-missing",
        type: "searchContext",
      }),
    ).toBe(false);
    expect(
      isWebviewIntent({
        data: {
          kind: "folder",
          query: "app",
          requestId: "req-2",
        },
        messageId: "search-bad-kind",
        type: "searchContext",
      }),
    ).toBe(false);
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

  it("filters malformed context search matches before the frontend consumes them", () => {
    expect(
      coerceContextSearchResultEvent({
        matches: [],
        query: "app",
        truncated: false,
        type: "contextSearchResult",
      }),
    ).toBeNull();

    expect(
      coerceContextSearchResultEvent({
        matches: [
          {
            description: "src",
            reference: {
              kind: "file",
              label: "app.ts",
              path: "src/app.ts",
              type: "reference",
            },
          },
          {
            description: "src",
            reference: {
              kind: "file",
              label: 123,
              path: "src/bad.ts",
              type: "reference",
            },
          },
          {
            description: 42,
            reference: {
              kind: "file",
              label: "also-bad.ts",
              path: "src/also-bad.ts",
              type: "reference",
            },
          },
        ],
        query: "app",
        requestId: "req-3",
        truncated: false,
        type: "contextSearchResult",
        workspaceAvailable: true,
      }),
    ).toEqual({
      matches: [
        {
          description: "src",
          reference: {
            kind: "file",
            label: "app.ts",
            path: "src/app.ts",
            type: "reference",
          },
        },
      ],
      query: "app",
      requestId: "req-3",
      truncated: false,
      type: "contextSearchResult",
      workspaceAvailable: true,
    });
  });

  it("drops unknown pending message ids", async () => {
    const tracker = new PendingMessageTracker<string>();
    const pending = tracker.create("known", 1_000);

    expect(tracker.resolve("unknown", "ignored")).toBe(false);
    expect(tracker.resolve("known", "done")).toBe(true);
    await expect(pending).resolves.toBe("done");
  });
});
