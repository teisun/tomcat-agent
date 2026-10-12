import { describe, expect, it, vi } from "vitest";
import { MessageQueue, type QueueContent } from "./messageQueue";

const content = (id: string): QueueContent => ({ userMessageId: id, text: id, segments: [], attachments: [] });
const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function fixture() {
  const conditions = { busy: true, commandPending: false, enabled: true };
  const send = vi.fn(async (_id: string, kind: string, _content: QueueContent): Promise<{ success: boolean; error?: string }> => {
    if (kind === "prompt") queue.start(_id);
    return { success: true };
  });
  const changed = vi.fn(), confirmed = vi.fn(), failed = vi.fn(), interrupt = vi.fn();
  const queue = new MessageQueue({ conditions: () => conditions, send, changed, confirmed, failed, interrupt });
  const idle = (outcome: "completed" | "interrupted" | "failed" = "completed") => { conditions.busy = false; queue.idle("s", outcome); };
  return { queue, conditions, send, changed, confirmed, failed, interrupt, idle };
}

describe("MessageQueue: volatile content and one send lane", () => {
  it("release keeps ordinary items paused but clears running state so reopening accepts new input", async () => {
    const f = fixture(); f.queue.start("s"); f.queue.enqueue("s", content("A"));
    f.queue.release("s"); f.conditions.busy = false;
    expect(f.queue.view("s").paused).toBe(true);
    expect(f.queue.isRunning("s")).toBe(false);
    const next = f.queue.submit("s", content("C"));
    expect(next.queued).toBe(false); expect(await next.completion).toBe(true);
    expect(f.queue.view("s").items.map(item => item.text)).toEqual(["A"]);
  });

  it("document reload cancels row editors and pauses without dropping queued content", () => {
    const f = fixture(); f.queue.enqueue("s", content("A")); f.queue.edit("s", "A");
    f.queue.cancelEditors();
    expect(f.queue.view("s").editingId).toBe(null);
    expect(f.queue.view("s").paused).toBe(true);
    expect(f.queue.view("s").items.map(item => item.text)).toEqual(["A"]);
  });

  it("busy inputs and FIFO dispatch carry content only", async () => {
    const f = fixture();
    f.queue.submit("s", content("X"));
    f.queue.submit("s", content("Y"));
    expect(f.send).not.toHaveBeenCalled();
    expect(f.queue.view("s").items.map(x => x.userMessageId)).toEqual(["X", "Y"]);
    expect(f.queue.view("s").items[0]).not.toHaveProperty("model");
    f.idle(); await flush();
    expect(f.send).toHaveBeenNthCalledWith(1, "s", "prompt", expect.objectContaining({ text: "X" }));
    expect(f.send).toHaveBeenCalledTimes(1);
    f.idle(); await flush();
    expect(f.send).toHaveBeenNthCalledWith(2, "s", "prompt", expect.objectContaining({ text: "Y" }));
  });

  it("IDLE explicit C runs before retained A/B, then A/B/D continue", async () => {
    const f = fixture();
    f.queue.enqueue("s", content("A")); f.queue.enqueue("s", content("B"));
    f.queue.stop("s"); f.idle("interrupted");
    const submitted = f.queue.submit("s", content("C"));
    expect(submitted.queued).toBe(false);
    expect(f.queue.view("s").items.map(x => x.text)).toEqual(["A", "B"]);
    expect(await submitted.completion).toBe(true);
    expect(f.queue.view("s").paused).toBe(false);
    f.queue.submit("s", content("D"));
    for (let i = 0; i < 3; i++) { f.idle(); await flush(); }
    expect(f.send.mock.calls.map(c => c[2].text)).toEqual(["C", "A", "B", "D"]);
  });

  it.each(["interrupted", "failed"] as const)("an external run unpauses after %s, then drains only after idle", async outcome => {
    const f = fixture(); f.queue.enqueue("s", content("A"));
    f.idle(outcome);
    expect(f.queue.view("s").paused).toBe(true);
    f.changed.mockClear();
    f.conditions.busy = true; f.queue.start("s");
    expect(f.queue.view("s").paused).toBe(false);
    expect(f.changed).toHaveBeenCalledWith("s");
    f.queue.dispatch("s"); await flush();
    expect(f.send).not.toHaveBeenCalled();
    f.idle(); await flush();
    expect(f.send).toHaveBeenCalledTimes(1);
    expect(f.send).toHaveBeenCalledWith("s", "prompt", expect.objectContaining({ text: "A" }));
  });

  it("a failed explicit C does not unpause or execute old work", async () => {
    const f = fixture(); f.queue.enqueue("s", content("A")); f.queue.stop("s"); f.idle("interrupted");
    f.send.mockResolvedValueOnce({ success: false, error: "bad config" });
    expect(await f.queue.submit("s", content("C")).completion).toBe(false);
    expect(f.queue.view("s").items.map(x => x.text)).toEqual(["A"]);
    expect(f.queue.view("s").paused).toBe(true);
    expect(f.send).toHaveBeenCalledTimes(1);
  });

  it("Stop discards a pending steer but retains normal work; late ACK cannot resurrect it", async () => {
    const f = fixture(); const ack = deferred<{success: boolean}>();
    f.send.mockImplementationOnce(() => ack.promise);
    f.queue.enqueue("s", content("X")); f.queue.enqueue("s", content("Y"));
    f.queue.sendNow("s", "X");
    expect(f.send).toHaveBeenCalledWith("s", "steer", expect.objectContaining({ text: "X" }));
    f.queue.stop("s"); f.idle("interrupted");
    ack.resolve({success: true}); await flush();
    f.queue.consumed("s", ["X"]);
    expect(f.queue.view("s").items.map(x => x.text)).toEqual(["Y"]);
    expect(f.queue.view("s").paused).toBe(true);
    expect(f.confirmed).not.toHaveBeenCalled();
    expect(f.send).toHaveBeenCalledTimes(1);
  });

  it("consumed exactly once removes the row and confirms history; queued ACK is not consumption", async () => {
    const f = fixture(); f.queue.enqueue("s", content("X")); f.queue.sendNow("s", "X"); await flush();
    expect(f.queue.view("s").items[0].status).toBe("steering");
    expect(f.confirmed).not.toHaveBeenCalled();
    f.queue.consumed("s", ["X"]); f.queue.consumed("s", ["X"]);
    expect(f.confirmed).toHaveBeenCalledTimes(1);
    expect(f.queue.view("s").items).toEqual([]);
  });

  it.each(["completed", "failed"] as const)("idle %s turns unconsumed steering back into ordinary head without reordering", async outcome => {
    const f = fixture(); f.queue.enqueue("s", content("Y")); f.queue.enqueue("s", content("X"));
    f.queue.sendNow("s", "X"); await flush();
    f.idle(outcome); await flush();
    if (outcome === "completed") expect(f.send.mock.calls.map(c => c[1])).toEqual(["steer", "prompt"]);
    else { expect(f.queue.view("s").items.map(x => x.text)).toEqual(["X", "Y"]); expect(f.queue.view("s").paused).toBe(true); }
  });

  it("editing head or a maintenance operation blocks FIFO; ending either rechecks", async () => {
    const f = fixture(); f.queue.enqueue("s", content("A")); f.queue.enqueue("s", content("B"));
    f.queue.edit("s", "A"); f.idle(); await flush(); expect(f.send).not.toHaveBeenCalled();
    f.conditions.commandPending = true; f.queue.save("s", { ...content("A"), text: "edited" });
    await flush(); expect(f.send).not.toHaveBeenCalled();
    f.conditions.commandPending = false; f.queue.dispatch("s"); f.queue.dispatch("s"); await flush();
    expect(f.send).toHaveBeenCalledTimes(1); expect(f.send.mock.calls[0][2].text).toBe("edited");
  });

  it("a duplicate idle during the next pending RPC never dispatches a second queued turn", async () => {
    const f = fixture(), ack = deferred<{ success: boolean }>();
    f.send.mockImplementationOnce(() => ack.promise);
    f.queue.enqueue("s", content("A")); f.queue.enqueue("s", content("B"));
    f.idle(); f.idle();
    ack.resolve({ success: true }); await flush();
    expect(f.send).toHaveBeenCalledTimes(1);
    expect(f.queue.view("s").items.map(item => item.text)).toEqual(["B"]);
    f.idle(); await flush(); expect(f.send).toHaveBeenCalledTimes(1);
    f.queue.start("s"); f.idle(); await flush(); expect(f.send).toHaveBeenCalledTimes(2);
  });

  it.each(["failed", "interrupted"] as const)("pre-start %s releases a starting prompt and keeps ordinary work paused", async outcome => {
    for (const idleBeforeAck of [false, true]) {
      const f = fixture(), ack = deferred<{success: boolean}>();
      f.conditions.busy = false; f.send.mockImplementationOnce(() => ack.promise);
      const first = f.queue.submit("s", content("A"));
      expect(f.queue.submit("s", content("B")).queued).toBe(true);
      if (idleBeforeAck) f.idle(outcome);
      ack.resolve({success: true}); await first.completion;
      if (!idleBeforeAck) f.idle(outcome);
      expect(f.queue.isRunning("s")).toBe(false);
      expect(f.queue.view("s").paused).toBe(true);
      expect(f.queue.view("s").items.map(item => item.text)).toEqual(["B"]);
      expect(f.send).toHaveBeenCalledTimes(1);
      expect(f.queue.submit("s", content("C")).queued).toBe(false);
      await flush();
      expect(f.send.mock.calls.map(call => call[2].text)).toEqual(["A", "C"]);
    }
  });

  it("explicit steering acknowledgement unpauses, but never sends ordinary work before idle", async () => {
    const f = fixture(); f.queue.enqueue("s", content("A")); f.queue.enqueue("s", content("B")); f.queue.pause("s");
    f.queue.sendNow("s", "A"); await flush();
    expect(f.queue.view("s").paused).toBe(false);
    expect(f.send).toHaveBeenCalledTimes(1);
  });

  it("in-flight Enter queues while an idle-before-ACK race never marks a finished run busy", async () => {
    const f = fixture(); f.conditions.busy = false;
    const ack = deferred<{success: boolean}>(); f.send.mockImplementationOnce(() => ack.promise);
    const first = f.queue.submit("s", content("A"));
    expect(f.queue.submit("s", content("B")).queued).toBe(true);
    f.queue.start("s"); f.idle();
    ack.resolve({success: true}); await first.completion; await flush();
    expect(f.send.mock.calls.map(c => c[2].text)).toEqual(["A", "B"]);
  });

  it("Stop before a prompt ACK keeps pause and supplements the ordinary interrupt", async () => {
    const f = fixture(); f.conditions.busy = false;
    const ack = deferred<{success: boolean}>(); f.send.mockImplementationOnce(() => ack.promise);
    const first = f.queue.submit("s", content("A"));
    f.queue.stop("s"); ack.resolve({success: true}); await first.completion;
    expect(f.interrupt).toHaveBeenCalledWith("s"); expect(f.queue.view("s").paused).toBe(true);
    expect(f.queue.hasInFlight("s")).toBe(false);
    f.queue.start("s");
    expect(f.queue.view("s").paused).toBe(true);
  });

  it("serve replacement discards all sessions and ignores old attempts", async () => {
    const f = fixture(); const ack = deferred<{success: boolean}>(); f.send.mockImplementationOnce(() => ack.promise);
    f.queue.enqueue("s", content("A")); f.queue.enqueue("background", content("B"));
    f.queue.sendNow("s", "A"); f.queue.reset();
    ack.resolve({success: false}); await flush();
    expect(f.queue.view("s").items).toEqual([]); expect(f.queue.view("background").items).toEqual([]);
    expect(f.failed).not.toHaveBeenCalled();
  });
});
