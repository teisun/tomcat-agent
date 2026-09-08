import { expect, it } from "vitest";
import { createHostE2eFixture } from "../scripts/e2eHostFixture";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import type { ControlRequestFrame } from "../src/serveClient/protocol";

type HistoryEntry = { id: string; message?: {
  content: unknown; role: string; superseded?: boolean; tool_call_id?: string;
  tool_calls?: Array<{ id: string }>;
} };

it("keeps distinct question calls durable across a host restart and settles the resumed call", async () => {
  const fixture = await createHostE2eFixture();
  const messenger = new TomcatMessenger({ executable: fixture.fakeServePath, env: fixture.env });
  const requests: ControlRequestFrame[] = [];
  messenger.onControlRequest((frame) => requests.push(frame));
  const history = async (sessionId: string) => {
    const response = await messenger.sendGetMessages(sessionId);
    expect(response.success).toBe(true);
    return (response.payload as { messages: HistoryEntry[] }).messages;
  };
  const answer = async (frame: ControlRequestFrame) => {
    messenger.sendControlResponse(frame.requestId, frame.sessionId, {
      requestId: frame.requestId,
      result: { answers: [], cancelled: true, outcome: "skipped" },
    });
    // Ordered get_state roundtrip observes the synchronous fixture's completed turn.
    const response = await messenger.request({ type: "get_state", sessionId: frame.sessionId });
    expect(response.payload).toMatchObject({ busy: false });
  };
  try {
    const initialized = await initializeServe(messenger);
    const sessionId = initialized.sessionId!;
    await messenger.request({ type: "prompt", sessionId, text: "answer card showcase" });
    await expect.poll(() => requests.length).toBe(1);
    await answer(requests[0]);
    await messenger.request({ type: "prompt", sessionId, text: "answer card showcase" });
    await expect.poll(() => requests.length).toBe(2);
    const first = requests[0].payload as { toolCallId: string };
    const second = requests[1].payload as { toolCallId: string };
    expect(second.toolCallId).not.toBe(first.toolCallId);
    expect(requests[1].requestId).not.toBe(requests[0].requestId);
    const before = await history(sessionId);
    expect(before.some((entry) => entry.message?.tool_calls?.some((tool) => tool.id === second.toolCallId))).toBe(true);
    expect(before.some((entry) => entry.message?.tool_call_id === second.toolCallId && entry.message.content === "[pending]")).toBe(true);

    messenger.restart();
    await initializeServe(messenger);
    await expect.poll(() => requests.length).toBe(3);
    expect(requests[2].requestId).not.toBe(requests[1].requestId);
    expect(requests[2].payload).toMatchObject({ toolCallId: second.toolCallId });
    expect(await history(sessionId)).toEqual(before);
    messenger.sendControlResponse(requests[1].requestId, sessionId, { result: { answers: [], cancelled: true } });
    expect((await messenger.request({ type: "get_state", sessionId })).payload).toMatchObject({ busy: true });
    await answer(requests[2]);
    const after = await history(sessionId);
    expect(new Set(after.map((entry) => entry.id)).size).toBe(after.length);
    expect(after.filter((entry) => entry.message?.tool_call_id === second.toolCallId && !entry.message.superseded)).toHaveLength(1);
    const accepted = await messenger.request({ type: "prompt", sessionId, text: "interrupt please" });
    expect(accepted.success).toBe(true);
    await messenger.request({ type: "interrupt", sessionId });
  } finally {
    await messenger.disposeAsync();
    await fixture.cleanup();
  }
});
