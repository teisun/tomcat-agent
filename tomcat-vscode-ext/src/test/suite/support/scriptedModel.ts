// Shared by Vitest and the compiled VS Code devhost tests. No paid model calls.
import * as http from "node:http";
import { setTimeout as delay } from "node:timers/promises";
import type { Socket } from "node:net";

export type ScriptedPart = { body: string; delayMs?: number; waitFor?: Promise<void> };
export type ScriptedResponse = { parts: ScriptedPart[]; status?: number };
export function sseDelta(content: string): ScriptedPart {
  return { body: `data: ${JSON.stringify({ choices: [{ delta: { content } }] })}\n\n` };
}
export function sseFinish(reason: string): ScriptedPart {
  return { body: `data: ${JSON.stringify({ choices: [{ finish_reason: reason }] })}\n\n` };
}
export function sseDone(): ScriptedPart { return { body: "data: [DONE]\n\n" }; }
export function sseToolCall(id: string, name: string, argsJson: string): ScriptedPart {
  return { body: `data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, id, function: { name, arguments: argsJson } }] } }] })}\n\n` };
}

export async function spawnScriptedOpenAiStreamServer(responses: ScriptedResponse[], listedModels: string[] = [], port = 0) {
  const captured: string[] = [];
  const modelListAuthorizations: string[] = [];
  let responseIndex = 0;
  const abort = new AbortController();
  const sockets = new Set<Socket>();
  const inFlight = new Set<Promise<void>>();
  let closePromise: Promise<void> | undefined;
  const server = http.createServer((request, response) => {
    const task = respond(request, response).catch((error: unknown) => {
      response.destroy(error instanceof Error ? error : new Error(String(error)));
    });
    inFlight.add(task);
    void task.then(() => inFlight.delete(task));
  });
  server.on("connection", socket => { sockets.add(socket); socket.once("close", () => sockets.delete(socket)); });
  async function respond(request: http.IncomingMessage, response: http.ServerResponse): Promise<void> {
    if (request.method === "GET" && request.url === "/_test/request-count") {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ count: captured.length, modelListAuthorizations, utilityRequests: captured.filter(isTitleRequest).map(utilityRequestSummary) }));
      return;
    }
    if (request.method === "GET" && request.url?.endsWith("/models")) {
      modelListAuthorizations.push(request.headers.authorization ?? "");
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ data: listedModels.map(id => ({ id, object: "model" })) }));
      return;
    }
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    const body = Buffer.concat(chunks).toString("utf8");
    const headers = Object.entries(request.headers).map(([key, value]) => `${key}: ${Array.isArray(value) ? value.join(",") : value ?? ""}`).join("\r\n");
    const rawRequest = `${request.method} ${request.url} HTTP/1.1\r\n${headers}\r\n\r\n${body}`;
    captured.push(rawRequest);
    if (isTitleRequest(rawRequest)) {
      response.writeHead(200, { Connection: "close", "Content-Type": "application/json" });
      response.end(sessionTitleResponseJson(rawRequest, "Generated title"));
      return;
    }
    const scripted = responses[responseIndex++];
    if (!scripted) { response.statusCode = 500; response.end("unexpected request"); return; }
    response.writeHead(scripted.status ?? 200, { "Cache-Control": "no-cache", Connection: "close", "Content-Type": scripted.status && scripted.status !== 200 ? "application/json" : "text/event-stream" });
    for (const part of scripted.parts) {
      if (part.waitFor) await part.waitFor;
      if (part.delayMs && part.delayMs > 0) await delay(part.delayMs, undefined, { signal: abort.signal });
      response.write(part.body);
    }
    response.end();
  }
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => { server.off("error", reject); resolve(); });
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("failed to bind scripted OpenAI server");
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    capturedRequests: () => [...captured],
    capturedNonTitleRequests: () => captured.filter(request => !isTitleRequest(request)),
    close() {
      if (closePromise) return closePromise;
      abort.abort();
      closePromise = (async () => {
        let timer: NodeJS.Timeout | undefined;
        try {
          await Promise.race([
            new Promise<void>((resolve, reject) => {
              server.close(error => error ? reject(error) : resolve());
              for (const socket of sockets) socket.destroy();
            }).then(() => Promise.all([...inFlight])),
            new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("scripted server cleanup timed out")), 3_000); }),
          ]);
        } finally { clearTimeout(timer); }
      })();
      return closePromise;
    },
  };
}
function isTitleRequest(rawRequest: string): boolean {
  const body = rawRequest.split("\r\n\r\n")[1];
  if (!body) return false;
  let value: unknown;
  try { value = JSON.parse(body); } catch { return false; }
  // Utility calls never send tools; Responses-API utility calls still stream.
  const hasTools = isRecord(value) && Array.isArray(value.tools) && value.tools.length > 0;
  return isRecord(value) && !hasTools && containsTitlePrompt(value);
}
const SESSION_TITLE_PROMPT = "Generate a short chat title from the user's first message.";
const TURN_SUMMARY_PROMPT = "Generate a short title summarizing this AI assistant thinking block and its tool usage.";
function containsText(value: unknown, text: string): boolean {
  if (typeof value === "string") return value.includes(text);
  if (Array.isArray(value)) return value.some(item => containsText(item, text));
  return isRecord(value) && Object.values(value).some(item => containsText(item, text));
}
function containsTitlePrompt(value: unknown): boolean {
  return containsText(value, SESSION_TITLE_PROMPT) || containsText(value, TURN_SUMMARY_PROMPT);
}
function utilityRequestSummary(rawRequest: string): { model: unknown; turnSummary: boolean } {
  const body: unknown = JSON.parse(rawRequest.split("\r\n\r\n")[1]);
  return { model: isRecord(body) ? body.model : undefined, turnSummary: containsText(body, TURN_SUMMARY_PROMPT) };
}
function sessionTitleResponseJson(rawRequest: string, title: string): string {
  if (rawRequest.startsWith("POST /v1/responses ")) return JSON.stringify({
    id: "title-mock", output: [{ content: [{ text: title, type: "output_text" }], type: "message" }],
    status: "completed", usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
  });
  return JSON.stringify({ choices: [{ finish_reason: "stop", index: 0, message: { content: title, role: "assistant" } }],
    id: "title-mock", usage: { completion_tokens: 1, prompt_tokens: 1, total_tokens: 2 } });
}
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }
