import { readFile, writeFile } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";
import { recoveryClock } from "../src/ui/webview/tests/errorRecoveryClock";
import { createRealServeMessenger, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, sseToolCall, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const failure = { status: 401, parts: [{ body: '{"error":{"type":"authentication_error","message":"invalid API key"}}' }] };
const done = { parts: [sseDelta("recovered successfully"), sseFinish("stop"), sseDone()] };
type Row = { id: string; type: string; message?: { role: string; content?: unknown; [key: string]: unknown } };

describe("real serve host-owned automatic recovery", () => {
  it.each(["retry", "resume"] as const)("automatically %s using durable original input/tool results without changing the foreground session", async action => {
    const server = await spawnScriptedOpenAiStreamServer(action === "retry" ? [failure, done] : [
      { parts: [sseToolCall("read-once", "read", '{"path":"sample.txt"}'), sseFinish("tool_calls"), sseDone()] },
      failure, failure, done,
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    let provider: TomcatWebviewViewProvider | undefined;
    try {
      const models = path.join(runtime.fixture.homePath, ".tomcat", "models.toml");
      await writeFile(models, (await readFile(models, "utf8")).replace("vision = false", "vision = true"));
      await writeFile(path.join(runtime.fixture.workspacePath, "sample.txt"), "TOOL_RESULT_KEEP_ME");
      const initialized = await initializeServe(runtime.messenger);
      const sessionId = initialized.sessionId!;
      const router = new SessionRouter(runtime.messenger, () => runtime.fixture.workspacePath);
      const time = recoveryClock();
      provider = new TomcatWebviewViewProvider({
        extensionUri: vscode.Uri.file(runtime.fixture.workspacePath), extensionMode: vscode.ExtensionMode.Development,
        getDefaultCwd: () => runtime.fixture.workspacePath, ide: {} as never,
        initialize: async () => initialized, messenger: runtime.messenger, sessionRouter: router,
        errorRecoveryClock: time.clock,
      });
      await provider.setServeConnectionState("ready");
      await provider.dispatchTestIntent({ type: "ready", messageId: "ready" });
      const commands = vi.spyOn(runtime.messenger, "request"); // Observe, do not replace the transport.
      const tools: string[] = [];
      const subscription = runtime.messenger.onEvent(event => { if (event.type === "tool_execution_end" && event.sessionId === sessionId) tools.push(event.toolName); });
      const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
      const svg = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>').toString("base64");
      const ingested = await runtime.messenger.request({ type: "ingest_attachment", sessionId, attachment: {
        kind: "image", mimeType: "image/svg+xml", filename: "icon.svg", dataBase64: svg, providerBase64: png, providerMimeType: "image/png",
      } });
      expect(ingested.success).toBe(true);
      const blob = ingested.payload as { blobSha: string; providerSha: string };
      const failed = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
      const prompt = await runtime.messenger.request({ type: "prompt", sessionId, text: "ORIGINAL_INPUT_KEEP_ME", params: {
        userMessageId: "original-user", attachments: [{ ...blob, kind: "image", filename: "icon.svg", mimeType: "image/svg+xml" }],
      } });
      expect(prompt.success, prompt.error ?? "prompt failed").toBe(true);
      await failed;
      await (provider as any).serveEventQueue;
      const firstWait = provider.currentState().sessionViews[sessionId].errorRecovery;
      expect(firstWait).toBeDefined();
      const before = (await router.getMessages(sessionId, { attachmentMode: "reference" })).messages as Row[];
      const beforeUsers = before.filter(row => row.message?.role === "user");
      expect(beforeUsers).toHaveLength(1);
      const other = await provider.createFreshSessionForTest(runtime.fixture.workspacePath);
      expect(other).not.toBe(sessionId);
      const idle = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
      time.advance(300_000);
      await idle; await (provider as any).serveEventQueue;
      if (action === "resume") {
        // The backend de-duplicates the repeated 401 error row: the same button must reopen.
        const next = provider.currentState().sessionViews[sessionId].errorRecovery;
        expect(next).toEqual({ errorId: firstWait!.errorId, dueAt: firstWait!.dueAt + 300_000 });
        const complete = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
        time.advance(300_000); await complete; await (provider as any).serveEventQueue;
      }
      const after = (await router.getMessages(sessionId, { attachmentMode: "reference" })).messages as Row[];
      const users = after.filter(row => row.message?.role === "user");
      expect(users).toHaveLength(action === "retry" ? 2 : 1);
      expect(provider.currentState().activeSessionId).toBe(other);
      expect(provider.currentState().sessionViews[sessionId].errorRecovery).toBeUndefined();
      expect(JSON.stringify(after)).toContain("recovered successfully");
      expect(tools.filter(name => name === "read")).toHaveLength(action === "resume" ? 1 : 0);
      expect(commands.mock.calls.filter(([command]) => command.type === "prompt")).toHaveLength(1);
      expect(commands.mock.calls.filter(([command]) => command.type === action)).toHaveLength(action === "retry" ? 1 : 2);
      expect(commands.mock.calls.some(([command]) => command.type === (action === "retry" ? "resume" : "retry"))).toBe(false);
      if (action === "retry") {
        expect(users[0].message).toMatchObject({ superseded: true, turn_failed: true });
        expect(users[1].id).not.toBe(users[0].id);
        expect(users[1].message?.content).toEqual(users[0].message?.content);
      } else {
        expect(users.map(row => row.id)).toEqual(beforeUsers.map(row => row.id));
      }
      for (const user of users) {
        const audit = JSON.stringify(user);
        expect(audit).toContain(blob.blobSha); expect(audit).toContain(blob.providerSha); expect(audit).toContain("image/svg+xml");
      }
      const requests = server.capturedNonTitleRequests().map(raw => JSON.parse(raw.split("\r\n\r\n")[1]));
      expect(requests).toHaveLength(action === "retry" ? 2 : 4);
      const last = requests.at(-1);
      expect(JSON.stringify(last).match(/ORIGINAL_INPUT_KEEP_ME/g)).toHaveLength(1);
      expect(JSON.stringify(last)).toContain(`data:image/png;base64,${png}`);
      if (action === "resume") expect(last.messages.filter((m: { role: string }) => m.role === "tool")).toEqual([expect.objectContaining({ tool_call_id: "read-once", content: expect.stringContaining("TOOL_RESULT_KEEP_ME") })]);
      subscription.dispose(); commands.mockRestore();
    } finally { provider?.dispose(); await runtime.cleanup(); await server.close(); }
  }, 60_000);
});
