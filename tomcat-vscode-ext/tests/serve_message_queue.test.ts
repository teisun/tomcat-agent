import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { MessageQueue, type ComposerConfig, type QueueContent } from "../src/ui/webview/messageQueue";
import { createRealServeMessenger, ensureTomcatBinary, spawnScriptedOpenAiStreamServer, sseDelta, sseDone, sseFinish, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const config: ComposerConfig = { agentMode: "chat", model: "gpt-5.4" };
const content = (text: string): QueueContent => ({ text, userMessageId: text, segments: [], attachments: [] });
function barrier() { let release!: () => void; const promise = new Promise<void>(resolve => { release = resolve; }); return { promise, release }; }
function bridge(messenger: TomcatMessenger, sessionId: string) {
  let busy = false, idleCount = 0;
  const acceptedSteer = barrier();
  const confirmed: string[] = [];
  const errors: string[] = [];
  const sent: Array<{ kind: string; id: string; params: unknown }> = [];
  const queue = new MessageQueue({
    conditions: () => ({ busy, enabled: true, commandPending: false }),
    changed: () => undefined,
    confirmed: (_session, input) => { confirmed.push(input.userMessageId); },
    failed: (_session, error) => { errors.push(error); },
    interrupt: id => { void messenger.request({ type: "interrupt", sessionId: id }); },
    send: async (id, kind, input, next) => {
      const selected = next ?? queue.config(id, config);
      const params = { userMessageId: input.userMessageId, segments: input.segments,
        ...(kind === "steer" ? { onlyIfRunning: true } : { agentMode: selected.agentMode, model: selected.model }) };
      sent.push({ kind, id: input.userMessageId, params });
      try { return await messenger.request({ type: kind, sessionId: id, text: input.text, params }); }
      finally { if (kind === "steer") acceptedSteer.release(); }
    },
  });
  const subscription = messenger.onEvent(event => {
    if (event.sessionId !== sessionId) return;
    if (event.type === "agent_start") { busy = true; queue.start(sessionId); }
    if (event.type === "steering_consumed") queue.consumed(sessionId, event.userMessageIds);
    if (event.type === "agent_idle") { busy = false; idleCount++; queue.idle(sessionId, event.outcome); }
  });
  return { queue, errors, confirmed, sent, acceptedSteer, idleCount: () => idleCount, dispose: () => subscription.dispose() };
}
const done = (text: string) => ({ parts: [sseDelta(text), sseFinish("stop"), sseDone()] });
const bodies = (raw: string[]) => raw.map(request => JSON.parse(request.split("\r\n\r\n")[1]));
const lastUser = (body: any) => body.messages.filter((m: any) => m.role === "user").at(-1).content;

describe("real serve message queue", () => {
  it("steering keeps A; completed FIFO turns use B's own thinking format on the same route", async () => {
    const gate = barrier();
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("held"), { ...sseFinish("stop"), waitFor: gate.promise }, sseDone()] },
      done("steered"), done("first queued task"), done("second queued task"),
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    let owner: ReturnType<typeof bridge> | undefined;
    try {
      await appendFile(path.join(runtime.fixture.homePath, ".tomcat", "models.toml"), `
[[models]]
id = "queue-B"
model_name = "queue-B-wire"
api = "openai"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "${server.baseUrl}"
thinking_format = "deepseek"
supported_reasoning_levels = ["high"]
context_window = 272000
max_output_tokens = 32768
capabilities = { vision = false, files = false, tools = true, reasoning = true, web_search = false }
`);
      const init = await initializeServe(runtime.messenger);
      expect(init.capabilities).toContain("message_queue"); const id = init.sessionId!;
      owner = bridge(runtime.messenger, id);
      const firstDelta = waitForEvent(runtime.messenger, e => e.type === "message_update" && e.sessionId === id);
      expect(await owner.queue.submit(id, content("initial-task"), config).completion).toBe(true); await firstDelta;
      owner.queue.submit(id, content("queued-A"), config); owner.queue.submit(id, content("queued-B"), config); owner.queue.submit(id, content("steer-X"), config);
      owner.queue.setConfig(id, { agentMode: "plan", model: "queue-B" });
      owner.queue.sendNow(id, "steer-X", owner.queue.config(id, config)); await owner.acceptedSteer.promise;
      expect(owner.sent.find(call => call.kind === "steer")!.params).toEqual({ userMessageId: "steer-X", segments: [], onlyIfRunning: true });
      const complete = waitForEvent(runtime.messenger, e => e.type === "agent_idle" && owner!.idleCount() === 3, 30000);
      gate.release(); await complete;
      const requests = bodies(server.capturedNonTitleRequests());
      expect(requests.map(r => r.model)).toEqual(["gpt-5.4", "gpt-5.4", "queue-B-wire", "queue-B-wire"]);
      expect(JSON.stringify(requests[1].messages)).toContain("steer-X");
      expect(requests[2].thinking.type).toBe("enabled"); expect(requests[3].thinking.type).toBe("enabled");
      expect(lastUser(requests[2])).toContain("queued-A"); expect(lastUser(requests[3])).toContain("queued-B");
      expect(owner.confirmed.filter(value => value === "steer-X")).toHaveLength(1);
      expect(owner.queue.view(id).items).toEqual([]);
    } finally { gate.release(); owner?.dispose(); await runtime.cleanup(); await server.close(); }
  }, 60000);

  it("Stop drops unconsumed steering, retains/pause ordinary work; idle C runs before A/B/D", async () => {
    const firstGate = barrier(), cGate = barrier();
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("held"), { ...sseFinish("stop"), waitFor: firstGate.promise }, sseDone()] },
      { parts: [sseDelta("new C"), { ...sseFinish("stop"), waitFor: cGate.promise }, sseDone()] },
      done("A done"), done("B done"), done("D done"),
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl); let owner: ReturnType<typeof bridge> | undefined;
    try {
      const init = await initializeServe(runtime.messenger); const id = init.sessionId!; owner = bridge(runtime.messenger, id);
      const started = waitForEvent(runtime.messenger, e => e.type === "message_update");
      await owner.queue.submit(id, content("initial-task"), config).completion; await started;
      for (const text of ["A", "B", "discarded-X"]) owner.queue.submit(id, content(text), config);
      owner.queue.sendNow(id, "discarded-X", config); await owner.acceptedSteer.promise;
      const stopped = waitForEvent(runtime.messenger, e => e.type === "agent_idle" && e.outcome === "interrupted");
      owner.queue.stop(id); expect((await runtime.messenger.request({ type: "interrupt", sessionId: id })).success).toBe(true); await stopped;
      firstGate.release();
      expect(owner.queue.view(id).items.map(item => item.text)).toEqual(["A", "B"]); expect(owner.queue.view(id).paused).toBe(true);
      const history = await runtime.messenger.request({ type: "get_messages", sessionId: id, params: {} });
      expect(JSON.stringify(history.payload)).not.toContain("discarded-X");
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
      const cStarted = waitForEvent(runtime.messenger, e => e.type === "message_update");
      const c = owner.queue.submit(id, content("C"), config); expect(c.queued).toBe(false); await c.completion; await cStarted;
      expect(owner.queue.view(id).items.map(item => item.text)).toEqual(["A", "B"]);
      owner.queue.submit(id, content("D"), config);
      const complete = waitForEvent(runtime.messenger, e => e.type === "agent_idle" && owner!.idleCount() === 5, 30000);
      cGate.release(); await complete;
      const requests = bodies(server.capturedNonTitleRequests());
      expect(requests.map(lastUser)).toEqual(["initial-task", "C", "A", "B", "D"]);
      expect(JSON.stringify(requests)).not.toContain("discarded-X");
    } finally { firstGate.release(); cGate.release(); owner?.dispose(); await runtime.cleanup(); await server.close(); }
  }, 60000);

  it("rich steering freezes instruction and reference content, archives SVG while requesting PNG only at consumption", async () => {
    const gate = barrier();
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [sseDelta("held"), { ...sseFinish("stop"), waitFor: gate.promise }, sseDone()] }, done("rich steer done"),
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    try {
      const models = path.join(runtime.fixture.homePath, ".tomcat", "models.toml");
      await writeFile(models, (await readFile(models, "utf8")).replace("vision = false", "vision = true"));
      const commands = path.join(runtime.fixture.workspacePath, ".cursor", "commands");
      await mkdir(commands, { recursive: true });
      const instruction = path.join(commands, "review.md");
      await writeFile(instruction, "FROZEN_COMMAND_BODY");
      const init = await initializeServe(runtime.messenger), sessionId = init.sessionId!;
      const svg = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>').toString("base64");
      const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
      const ingested = await runtime.messenger.request({ type: "ingest_attachment", sessionId, attachment: {
        kind: "image", mimeType: "image/svg+xml", filename: "icon.svg", dataBase64: svg, providerBase64: png, providerMimeType: "image/png",
      } });
      expect(ingested.success).toBe(true);
      const blob = ingested.payload as { blobSha: string; providerSha: string };
      const started = waitForEvent(runtime.messenger, event => event.type === "message_update");
      expect((await runtime.messenger.request({ type: "prompt", sessionId, text: "hold initial task", params: {} })).success).toBe(true);
      await started;
      const rich = await runtime.messenger.request({ type: "steer", sessionId, text: "Inspect selected source", params: {
        onlyIfRunning: true, userMessageId: "rich-steer", attachments: [{ ...blob, kind: "image", filename: "icon.svg", mimeType: "image/svg+xml" }],
        segments: [
          { type: "text", text: "Inspect selected source" },
          { type: "reference", kind: "selection", label: "selected.ts:1", path: "selected.ts", text: "SELECTED_SOURCE", startLine: 1, endLine: 1 },
          { type: "instruction", kind: "command", resourceId: "command:.cursor/commands/review.md", label: "/review" },
        ],
      } });
      expect(rich.success, rich.error ?? "steer rejected").toBe(true);
      const before = await runtime.messenger.request({ type: "get_messages", sessionId });
      expect(JSON.stringify(before.payload)).not.toContain("rich-steer");
      await writeFile(instruction, "CHANGED_COMMAND_BODY");
      const ended = waitForEvent(runtime.messenger, event => event.type === "agent_idle");
      gate.release();
      const events = await ended;
      expect(events.filter(event => event.type === "steering_consumed")).toEqual([expect.objectContaining({ userMessageIds: ["rich-steer"] })]);
      const request = JSON.stringify(bodies(server.capturedNonTitleRequests())[1]);
      for (const value of ["Inspect selected source", "SELECTED_SOURCE", "FROZEN_COMMAND_BODY", `data:image/png;base64,${png}`]) expect(request).toContain(value);
      expect(request).not.toContain("CHANGED_COMMAND_BODY"); expect(request).not.toContain(svg);
      const history = await runtime.messenger.request({ type: "get_messages", sessionId, params: { attachmentMode: "reference" } });
      const rows = (history.payload as { messages: Array<{ id: string; message: { content: unknown; kind?: string } }> }).messages.filter(row => row.id === "rich-steer");
      expect(rows).toHaveLength(1); expect(rows[0].message.kind).toBe("steering");
      const original = JSON.stringify(rows[0]);
      for (const value of [blob.blobSha, blob.providerSha, "image/svg+xml", "FROZEN_COMMAND_BODY", "SELECTED_SOURCE"]) expect(original).toContain(value);
    } finally { gate.release(); await runtime.cleanup(); await server.close(); }
  }, 60000);

  it("serve replacement drops all queues and never replays them while durable history remains", async () => {
    const gate = barrier();
    const server = await spawnScriptedOpenAiStreamServer([{ parts: [sseDelta("held"), { ...sseFinish("stop"), waitFor: gate.promise }, sseDone()] }, done("new turn")]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    let replacement: TomcatMessenger | undefined; let owner: ReturnType<typeof bridge> | undefined;
    try {
      const init = await initializeServe(runtime.messenger); const id = init.sessionId!; owner = bridge(runtime.messenger, id);
      const started = waitForEvent(runtime.messenger, e => e.type === "message_update");
      await owner.queue.submit(id, content("durable-initial"), config).completion; await started;
      owner.queue.submit(id, content("never-replay-A"), config); owner.queue.enqueue("background", content("never-replay-B"));
      owner.queue.reset(); owner.dispose(); await runtime.messenger.disposeAsync(); gate.release();
      expect(owner.queue.view(id).items).toEqual([]); expect(owner.queue.view("background").items).toEqual([]);
      replacement = new TomcatMessenger({ executable: await ensureTomcatBinary(), cwd: runtime.fixture.workspacePath, env: runtime.fixture.env, requestTimeoutMs: 10000 });
      const after = await initializeServe(replacement);
      const history = await replacement.request({ type: "get_messages", sessionId: after.sessionId, params: {} });
      expect(JSON.stringify(history.payload)).toContain("durable-initial"); expect(JSON.stringify(history.payload)).not.toContain("never-replay");
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
      const complete = waitForEvent(replacement, e => e.type === "agent_idle");
      expect((await replacement.request({ type: "prompt", sessionId: after.sessionId, text: "new turn", params: {} })).success).toBe(true); await complete;
      expect(server.capturedNonTitleRequests()).toHaveLength(2);
      expect(JSON.stringify(bodies(server.capturedNonTitleRequests()))).not.toContain("never-replay");
    } finally { gate.release(); owner?.dispose(); await replacement?.disposeAsync(); await runtime.cleanup(); await server.close(); }
  }, 60000);
});
