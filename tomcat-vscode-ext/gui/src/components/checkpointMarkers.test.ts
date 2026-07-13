import { describe, expect, it } from "vitest";

import type { WebviewCheckpoint, WebviewTimelineItem } from "../types";
import { injectCheckpointMarkers } from "./checkpointMarkers";

describe("injectCheckpointMarkers", () => {
  it("injects checkpoint markers before the next user message only", () => {
    const timeline: WebviewTimelineItem[] = [
      { id: "user-1", kind: "user", text: "first prompt", type: "message" },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "first reply",
        type: "message",
      },
      { id: "user-2", kind: "user", text: "second prompt", type: "message" },
      {
        assistantMessageId: "assistant-2",
        id: "assistant-2",
        kind: "assistant",
        text: "second reply",
        type: "message",
      },
    ];
    const checkpoints: WebviewCheckpoint[] = [
      {
        changedFiles: ["src/two.ts"],
        createdAt: "2026-07-12T12:05:00Z",
        id: "ck-2",
        kind: "turn_end",
        messageAnchor: "assistant-2",
      },
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ];

    expect(
      injectCheckpointMarkers(timeline, checkpoints).map((item) =>
        item.type === "checkpoint" ? item.checkpointId : item.id,
      ),
    ).toEqual(["user-1", "assistant-1", "ck-1", "user-2", "assistant-2"]);
  });

  it("falls back to ${anchor}-thinking when the assistant rendered only a thinking block", () => {
    const timeline: WebviewTimelineItem[] = [
      { id: "user-1", kind: "user", text: "first prompt", type: "message" },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1-thinking",
        text: "checking files",
        type: "thinking",
      },
      { id: "user-2", kind: "user", text: "second prompt", type: "message" },
    ];

    expect(
      injectCheckpointMarkers(timeline, [
        {
          changedFiles: ["src/one.ts"],
          createdAt: "2026-07-12T12:00:00Z",
          id: "ck-thinking",
          kind: "turn_end",
          messageAnchor: "assistant-1",
        },
      ]).map((item) => (item.type === "checkpoint" ? item.checkpointId : item.id)),
    ).toEqual(["user-1", "assistant-1-thinking", "ck-thinking", "user-2"]);
  });

  it("skips the latest checkpoint when there is no following user message", () => {
    const timeline: WebviewTimelineItem[] = [
      { id: "user-1", kind: "user", text: "first prompt", type: "message" },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "first reply",
        type: "message",
      },
    ];

    expect(
      injectCheckpointMarkers(timeline, [
        {
          changedFiles: ["src/one.ts"],
          createdAt: "2026-07-12T12:00:00Z",
          id: "ck-last",
          kind: "turn_end",
          messageAnchor: "assistant-1",
        },
      ]),
    ).toEqual(timeline);
  });

  it("is idempotent when reinjecting into an already injected timeline", () => {
    const timeline: WebviewTimelineItem[] = [
      { id: "user-1", kind: "user", text: "first prompt", type: "message" },
      {
        assistantMessageId: "assistant-1",
        id: "assistant-1",
        kind: "assistant",
        text: "first reply",
        type: "message",
      },
      { id: "user-2", kind: "user", text: "second prompt", type: "message" },
    ];
    const checkpoints: WebviewCheckpoint[] = [
      {
        changedFiles: ["src/one.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        messageAnchor: "assistant-1",
      },
    ];

    const once = injectCheckpointMarkers(timeline, checkpoints);
    const twice = injectCheckpointMarkers(once, checkpoints);

    expect(twice).toEqual(once);
  });
});
