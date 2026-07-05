import { describe, expect, it } from "vitest";

import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

describe("TomcatMessenger NDJSON framing", () => {
  it("reassembles split frames and skips blank lines", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
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
  });
});
