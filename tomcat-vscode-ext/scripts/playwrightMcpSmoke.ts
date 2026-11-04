import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as http from "node:http";
import * as path from "node:path";
import { randomUUID } from "node:crypto";
import { sseDelta, sseDone, sseFinish, sseToolCall, type ScriptedResponse } from "./realServeFixture";

// Also accepts the immutable inputs saved before a post-processing failure.
// Rechecking those inputs must not launch a browser or manufacture a new marker.
export async function recordPlaywrightMcpSmoke(requests: string[], proof: { marker: string; url: string; visits: number }, artifacts: string) {
  const { marker, url, visits } = proof;
  const bodies = requests.map(raw => JSON.parse(raw.slice(raw.indexOf("\r\n\r\n") + 4)));
  const messages = bodies.at(-1)?.messages as Array<{ role: string; tool_call_id?: string; content: unknown }> | undefined;
  assert.ok(messages, "The last model request must contain real MCP results");
  const results = messages.filter(message => message.role === "tool");
  await fs.writeFile(path.join(artifacts, "mcp-tool-results.json"), JSON.stringify({ marker, url, visits, results }, null, 2));
  assert.ok(visits > 0, "The real browser must visit the proof page");
  const text = (id: string) => String(results.find(message => message.tool_call_id === id)?.content);
  // Navigate may return a snapshot file link rather than inline page content.
  assert.ok(text("mcp-navigate").includes(`- Page URL: ${url}/`), "Navigation must reach the proof page URL");
  assert.ok(text("mcp-snapshot").includes(marker), "Snapshot must return this run's unpredictable page marker");
  const images: string[] = [];
  const scan = (value: unknown): void => {
    if (typeof value === "string" && value.startsWith("data:image/png;base64,")) images.push(value);
    else if (Array.isArray(value)) value.forEach(scan);
    else if (value && typeof value === "object") Object.values(value).forEach(scan);
  };
  scan(messages);
  assert.ok(images.length > 0, "The real MCP screenshot must reach the model as PNG image content");
  const png = Buffer.from(images.at(-1)!.split(",")[1], "base64");
  assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
  await fs.writeFile(path.join(artifacts, "mcp-browser.png"), png);
  await fs.writeFile(path.join(artifacts, "mcp-browser.aria.txt"), text("mcp-snapshot"));
  await fs.writeFile(path.join(artifacts, "mcp-browser.console.json"), JSON.stringify({ capturedBy: "browser_console_messages", result: text("mcp-console") }, null, 2));
  assert.match(text("mcp-console"), /Total messages: 0 \(Errors: 0, Warnings: 0\)/, "The proof page console must be clean");
  await fs.writeFile(path.join(artifacts, "mcp-smoke.json"), JSON.stringify({ status: "passed", marker, url, visits, imageBytes: png.length, model: "scripted locally; MCP and browser are real" }, null, 2));
}

// Only the model's instructions are scripted. Every MCP result must come back
// through the installed extension's real Serve and default Playwright process.
export async function createPlaywrightMcpSmoke() {
  const marker = `MCP_PROOF_${randomUUID()}`;
  let visits = 0;
  const page = http.createServer((request, response) => {
    if (request.url === "/favicon.ico") { response.writeHead(204); response.end(); return; }
    visits++;
    response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    response.end(`<!doctype html><html lang="en"><title>Tomcat MCP acceptance</title>
      <style>body{font:20px system-ui;background:#102030;color:#eef5ff;padding:64px}main{max-width:900px}h1{color:#67e8b3}code{display:block;padding:24px;background:#20364a;overflow-wrap:anywhere}</style>
      <main><h1>Built-in Playwright MCP is working</h1><p>This page was opened by the installed Tomcat extension.</p><code>${marker}</code></main></html>`);
  });
  await new Promise<void>((resolve, reject) => {
    page.once("error", reject);
    page.listen(0, "127.0.0.1", () => { page.off("error", reject); resolve(); });
  });
  const address = page.address();
  assert.ok(address && typeof address !== "string");
  const url = `http://127.0.0.1:${address.port}`;
  const tools = ["browser_navigate", "browser_snapshot", "browser_take_screenshot", "browser_console_messages", "browser_close"];
  const steps: Array<[string, string, Record<string, unknown>]> = [
    ["mcp-search", "tool_search", { source: "playwright" }],
    ["mcp-describe", "tool_describe", { names: tools.map(name => `mcp__playwright__${name}`) }],
    ["mcp-navigate", "tool_call", { name: "mcp__playwright__browser_navigate", arguments: { url } }],
    ["mcp-snapshot", "tool_call", { name: "mcp__playwright__browser_snapshot", arguments: {} }],
    // No filename/fullPage: require the real image to flow back to the model.
    ["mcp-screenshot", "tool_call", { name: "mcp__playwright__browser_take_screenshot", arguments: { scale: "css" } }],
    ["mcp-console", "tool_call", { name: "mcp__playwright__browser_console_messages", arguments: { level: "error" } }],
    ["mcp-close", "tool_call", { name: "mcp__playwright__browser_close", arguments: {} }],
  ];
  const responses: ScriptedResponse[] = steps.map(([id, name, args]) => ({
    parts: [sseToolCall(id, name, JSON.stringify(args)), sseFinish("tool_calls"), sseDone()],
  }));
  responses.push({ parts: [sseDelta("MCP_BROWSER_SMOKE_COMPLETE"), sseFinish("stop"), sseDone()] });
  return {
    responses,
    record: (requests: string[], artifacts: string) => recordPlaywrightMcpSmoke(requests, { marker, url, visits }, artifacts),
    async close() {
      await new Promise<void>((resolve, reject) => { page.close(error => error ? reject(error) : resolve()); page.closeAllConnections(); });
    },
  };
}
