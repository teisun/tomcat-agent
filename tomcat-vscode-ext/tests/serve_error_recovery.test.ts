import { readFile, writeFile } from "node:fs/promises";
import * as path from "node:path";
import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";
import { recoveryClock } from "../src/ui/webview/tests/errorRecoveryClock";
import { createRealServeMessenger, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, waitForEvent, warmTomcatBinaryForSuite } from "./serveTestUtils";

warmTomcatBinaryForSuite();
const failure = { status: 401, parts: [{ body: '{"error":{"type":"authentication_error","message":"invalid API key"}}' }] };
const done = { parts: [sseDelta("recovered successfully"), sseFinish("stop"), sseDone()] };
type Row = { id: string; type: string; message?: { role: string; content?: unknown; [key: string]: unknown } };

describe("real serve host-owned automatic recovery", () => {
  it.each([
    { action: "retry" as const, longTail: false },
    { action: "resume" as const, longTail: false },
    { action: "resume" as const, longTail: true },
  ])("automatically $action (long transient tail=$longTail) without changing the foreground session", async ({ action, longTail }) => {
    const toolCount = longTail ? 79 : 1;
    const filename = (index: number) => index === 0 ? "sample.txt" : `sample-${index}.txt`;
    const toolCalls = Array.from({ length: toolCount }, (_, index) => ({
      index, id: index === 0 ? "read-once" : `read-once-${index}`,
      function: { name: "read", arguments: JSON.stringify({ path: filename(index) }) },
    }));
    // Many small tool rounds reproduce the paginated turn; one 79-call batch
    // exceeds the existing append validator's 64-entry tail window instead.
    const toolRounds = Array.from({ length: Math.ceil(toolCount / 3) }, (_, round) => ({ parts: [
      { body: `data: ${JSON.stringify({ choices: [{ delta: {
        tool_calls: toolCalls.slice(round * 3, round * 3 + 3).map((call, index) => ({ ...call, index })),
      } }] })}\n\n` },
      sseFinish("tool_calls"), sseDone(),
    ] }));
    const overloaded = { status: 503, parts: [{ body: '{"error":{"type":"server_error","message":"temporarily overloaded"}}' }] };
    const server = await spawnScriptedOpenAiStreamServer(action === "retry" ? [failure, done] :
      longTail ? [...toolRounds, ...Array.from({ length: 10 }, () => overloaded), done] :
        [...toolRounds, failure, failure, done]);
    const runtime = await createRealServeMessenger(server.baseUrl);
    let provider: TomcatWebviewViewProvider | undefined;
    try {
      const models = path.join(runtime.fixture.homePath, ".tomcat", "models.toml");
      await writeFile(models, (await readFile(models, "utf8")).replace("vision = false", "vision = true"));
      await Promise.all(Array.from({ length: toolCount }, (_, index) =>
        writeFile(path.join(runtime.fixture.workspacePath, filename(index)), "TOOL_RESULT_KEEP_ME")));
      if (longTail) {
        // Keep the real default attempt budget; remove provider-level retries and
        // shorten only backoff in this disposable fixture so 10 attempts are observable.
        const configPath = path.join(runtime.fixture.homePath, ".tomcat", "tomcat.config.toml");
        let config = await readFile(configPath, "utf8");
        for (const key of ["retry_count", "agent_retry_base_delay_ms"]) {
          const pattern = new RegExp(`^${key}\\s*=.*$`, "m");
          config = pattern.test(config) ? config.replace(pattern, `${key} = 0`)
            : config.replace("[llm]", `[llm]\n${key} = 0`);
        }
        await writeFile(configPath, config);
      }
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
      const toolIds: string[] = [];
      const retryStarts: Array<{ attempt: number; maxAttempts: number }> = [];
      const subscription = runtime.messenger.onEvent(event => {
        if (event.sessionId !== sessionId) return;
        if (event.type === "tool_execution_end") { tools.push(event.toolName); toolIds.push(event.toolCallId); }
        if (event.type === "auto_retry_start") retryStarts.push({ attempt: event.attempt, maxAttempts: event.maxAttempts });
      });
      const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
      const svg = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>').toString("base64");
      const ingested = await runtime.messenger.request({ type: "ingest_attachment", sessionId, attachment: {
        kind: "image", mimeType: "image/svg+xml", filename: "icon.svg", dataBase64: svg, providerBase64: png, providerMimeType: "image/png",
      } });
      expect(ingested.success).toBe(true);
      const blob = ingested.payload as { blobSha: string; providerSha: string };
      const failed = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId, 60_000);
      const prompt = await runtime.messenger.request({ type: "prompt", sessionId, text: "ORIGINAL_INPUT_KEEP_ME", params: {
        userMessageId: "original-user", attachments: [{ ...blob, kind: "image", filename: "icon.svg", mimeType: "image/svg+xml" }],
      } });
      expect(prompt.success, prompt.error ?? "prompt failed").toBe(true);
      await failed;
      await (provider as any).serveEventQueue;
      const firstWait = provider.currentState().sessionViews[sessionId].errorRecovery;
      const before = (await router.getMessages(sessionId, { limit: 500, attachmentMode: "reference" })).messages as Row[];
      expect(firstWait, JSON.stringify({
        tools: tools.length, retryStarts, tail: before.slice(-6),
        errors: provider.currentState().sessionViews[sessionId].timeline.filter(item => item.type === "message" && item.kind === "error"),
      })).toBeDefined();
      if (longTail) {
        const page = await router.getMessages(sessionId, { limit: 80, attachmentMode: "reference" });
        expect(page.hasMore).toBe(true);
        expect((page.messages as Row[]).some(row => row.message?.role === "user")).toBe(false);
        const rows = page.messages as Row[];
        expect(rows.findLast(row => row.message?.role === "assistant")?.message?.tool_calls).toHaveLength(1);
        expect(rows.findLast(row => row.message?.role === "tool")?.message?.tool_call_id).toBe("read-once-78");
        expect(provider.currentState().sessionViews[sessionId].timeline).toContainEqual(
          expect.objectContaining({ kind: "error", recoveryAction: "resume" }),
        );
        expect(retryStarts).toEqual(Array.from({ length: 9 }, (_, index) => ({ attempt: index + 2, maxAttempts: 10 })));
        expect(server.capturedNonTitleRequests()).toHaveLength(toolRounds.length + 10);
      }
      const beforeUsers = before.filter(row => row.message?.role === "user");
      expect(beforeUsers).toHaveLength(1);
      const other = await provider.createFreshSessionForTest(runtime.fixture.workspacePath);
      expect(other).not.toBe(sessionId);
      const idle = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
      time.advance(300_000);
      await idle; await (provider as any).serveEventQueue;
      if (action === "resume" && !longTail) {
        // The backend de-duplicates the repeated 401 error row: the same button must reopen.
        const next = provider.currentState().sessionViews[sessionId].errorRecovery;
        expect(next).toEqual({ errorId: firstWait!.errorId, dueAt: firstWait!.dueAt + 300_000 });
        const complete = waitForEvent(runtime.messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
        time.advance(300_000); await complete; await (provider as any).serveEventQueue;
      }
      const after = (await router.getMessages(sessionId, { limit: 500, attachmentMode: "reference" })).messages as Row[];
      const users = after.filter(row => row.message?.role === "user");
      expect(users).toHaveLength(action === "retry" ? 2 : 1);
      expect(provider.currentState().activeSessionId).toBe(other);
      expect(provider.currentState().sessionViews[sessionId].errorRecovery).toBeUndefined();
      expect(JSON.stringify(after)).toContain("recovered successfully");
      expect(tools.filter(name => name === "read")).toHaveLength(action === "resume" ? toolCount : 0);
      expect(new Set(toolIds).size).toBe(toolIds.length);
      expect(commands.mock.calls.filter(([command]) => command.type === "prompt")).toHaveLength(1);
      expect(commands.mock.calls.filter(([command]) => command.type === action)).toHaveLength(action === "retry" || longTail ? 1 : 2);
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
      expect(requests).toHaveLength(longTail ? toolRounds.length + 11 : action === "retry" ? 2 : 4);
      const last = requests.at(-1);
      expect(JSON.stringify(last).match(/ORIGINAL_INPUT_KEEP_ME/g)).toHaveLength(1);
      expect(JSON.stringify(last)).toContain(`data:image/png;base64,${png}`);
      if (action === "resume") {
        const results = last.messages.filter((m: { role: string }) => m.role === "tool");
        expect(results).toHaveLength(toolCount);
        expect(results).toContainEqual(expect.objectContaining({ tool_call_id: "read-once", content: expect.stringContaining("TOOL_RESULT_KEEP_ME") }));
      }
      subscription.dispose(); commands.mockRestore();
    } finally { provider?.dispose(); await runtime.cleanup(); await server.close(); }
  }, 90_000);
});
