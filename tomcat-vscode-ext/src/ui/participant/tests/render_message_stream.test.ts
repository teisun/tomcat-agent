import { describe, expect, it } from "vitest";

import { ParticipantTurnRenderer } from "../render";

type MockStream = {
  anchorCalls: Array<{ label: string }>;
  anchors: Array<{ uri: unknown; label: string }>;
  buttons: Array<{ arguments?: unknown[]; command: string; title: string }>;
  markdowns: string[];
  progresses: string[];
  anchor(uri: unknown, label: string): void;
  button(payload: { arguments?: unknown[]; command: string; title: string }): void;
  markdown(value: string): void;
  progress(value: string): void;
};

function createStream(): MockStream {
  const stream: MockStream = {
    anchor: (uri, label) => {
      stream.anchors.push({ label, uri });
    },
    anchorCalls: [],
    anchors: [],
    button: (payload) => {
      stream.buttons.push(payload);
    },
    buttons: [],
    markdown: (value) => {
      stream.markdowns.push(value);
    },
    markdowns: [],
    progress: (value) => {
      stream.progresses.push(value);
    },
    progresses: [],
  };
  return stream;
}

describe("ParticipantTurnRenderer", () => {
  it("streams content and only announces thinking once", async () => {
    const stream = createStream();
    const renderer = new ParticipantTurnRenderer(
      {
        async rememberToolResult() {
          throw new Error("not used");
        },
        async rememberToolStart() {
          return undefined;
        },
      } as never,
      stream as never,
    );

    await renderer.render({ sessionId: "s1", type: "agent_start" } as never);
    await renderer.render({
      assistantMessageEvent: { delta: "hi", kind: "content_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    } as never);
    await renderer.render({
      assistantMessageEvent: { delta: "thought-1", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    } as never);
    await renderer.render({
      assistantMessageEvent: { delta: "thought-2", kind: "thinking_delta" },
      message: {},
      sessionId: "s1",
      type: "message_update",
    } as never);

    expect(stream.progresses).toEqual([
      "Tomcat agent started",
      "Tomcat is thinking...",
    ]);
    expect(stream.markdowns).toEqual(["hi"]);
  });

  it("renders file tool results with anchors and diff/apply buttons", async () => {
    const stream = createStream();
    const renderer = new ParticipantTurnRenderer(
      {
        createFileAnchor: (displayPath: string) => ({ displayPath }),
        async rememberToolResult(_toolCallId: string, displayPath: string) {
          return {
            absolutePath: `/workspace/${displayPath}`,
            displayPath,
            existedBefore: true,
            originalContent: "before",
            proposedContent: "after",
            toolCallId: "tool-1",
          };
        },
        async rememberToolStart() {
          return undefined;
        },
      } as never,
      stream as never,
    );

    await renderer.render({
      display: { file: "src/file.ts", kind: "file" },
      isError: false,
      result: { ok: true },
      sessionId: "s1",
      toolCallId: "tool-1",
      toolName: "write",
      type: "tool_execution_end",
    } as never);

    expect(stream.progresses).toContain("write finished");
    expect(stream.anchors).toEqual([
      { label: "src/file.ts", uri: { displayPath: "src/file.ts" } },
    ]);
    expect(stream.buttons).toEqual([
      {
        arguments: [{ toolCallId: "tool-1" }],
        command: "tomcat.openDiff",
        title: "Open Diff",
      },
      {
        arguments: [{ toolCallId: "tool-1" }],
        command: "tomcat.applyEdit",
        title: "Apply Edit",
      },
    ]);
  });
});
