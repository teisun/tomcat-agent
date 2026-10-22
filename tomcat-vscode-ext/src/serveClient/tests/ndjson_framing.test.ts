import { describe, expect, it } from "vitest";

import { createReadyMessenger } from "./fakes";
import { createConnectionFactory, FakeConnection } from "./fakes";

describe("TomcatMessenger NDJSON framing", () => {
  it("reassembles split frames and skips blank lines", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
    });
    const eventTypes: string[] = [];

    messenger.onEvent((event) => {
      eventTypes.push(event.type);
    });

    messenger.start();
    child.emitStdout(
      '{"type":"agent_start","sessionId":"s1"}\n{"type":"message_update","sessionId":"s1","assistantMessageEvent":{"kind":"content_delta","delta":"hel"},"message":{',
    );
    child.emitStdout('}}\n\n{"type":"agent_end","sessionId":"s1","messages":[],"error":null}\n');

    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(eventTypes).toEqual(["agent_start", "message_update", "agent_end"]);
    await messenger.disposeAsync();
  });

  it("preserves UTF-8 when socket chunks split a multi-byte character", async () => {
    const socket = new FakeConnection();
    const messenger = await createReadyMessenger({ executable: "unused", connectFactory: createConnectionFactory(socket) });
    const events: unknown[] = [];
    messenger.onEvent(event => events.push(event));
    const text = JSON.stringify({ type: "message_update", sessionId: "s1", message: { role: "assistant", content: "你好 👋" } }) + "\n";
    for (const byte of Buffer.from(text)) socket.emitStdout(Buffer.from([byte]));
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(events).toEqual([JSON.parse(text)]);
    await messenger.disposeAsync();
  });
});
