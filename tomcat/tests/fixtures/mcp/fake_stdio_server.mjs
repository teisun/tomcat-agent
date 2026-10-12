#!/usr/bin/env node

import readline from "node:readline";
import { access, appendFile, open } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";

const TINY_PNG_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9p8qAAAAAASUVORK5CYII=";

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const mode = process.argv.includes("--hang")
  ? "hang"
  : process.argv.includes("--die-midcall")
    ? "die-midcall"
    : process.argv.includes("--error")
      ? "error"
      : "normal";
const hangStartup = process.argv.includes("--hang-startup");
const option = (name) => {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
};
const recordPath = option("--record");
const eventsPath = option("--events");
const exitAfterCatalogMs = Number(option("--exit-after-catalog-ms") ?? 0);
const exitGateFile = option("--exit-gate-file");
const exitOnceFile = option("--exit-once-file");
const startupGateFile = option("--startup-gate-file");
const failStartupFile = option("--fail-startup-file");
const pending = new Map();
let writes = Promise.resolve();

// One write per JSON frame, even with many calls completing simultaneously.
function send(message) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`);
}
function reply(id, result) { send({ id, result }); }
function record(method) {
  if (recordPath) writes = writes.then(() => appendFile(recordPath, `${method}\n`));
  return writes;
}
function event(kind, message) {
  if (eventsPath) {
    const row = { kind, id: message.id, name: message.params?.name,
      label: message.params?.arguments?.label, progressToken: message.params?._meta?.progressToken };
    writes = writes.then(() => appendFile(eventsPath, `${JSON.stringify(row)}\n`));
  }
  return writes;
}

async function call(message) {
  const controller = new AbortController();
  const key = JSON.stringify(message.id);
  pending.set(key, controller);
  const args = message.params.arguments ?? {};
  let ticker;
  let response;
  try {
    await record("tools/call");
    await event("started", message);
    if (mode === "die-midcall") process.exit(0);
    if (args.progressMs > 0) {
      let progress = 0;
      ticker = setInterval(() => {
        send({ method: "notifications/progress", params: {
          progressToken: args.wrongToken ? "unrelated-token" : message.params._meta?.progressToken,
          progress: ++progress,
        } });
      }, args.progressMs);
    }
    if (mode === "hang") {
      if (!controller.signal.aborted) {
        await new Promise((resolve) => controller.signal.addEventListener("abort", resolve, { once: true }));
      }
    } else {
      if (args.delayMs > 0) await delay(args.delayMs, undefined, { signal: controller.signal });
      // File gates are test-only: the parent releases a request after observing B.
      while (args.gateFile && !controller.signal.aborted) {
        try { await access(args.gateFile); break; } catch {
          await delay(10, undefined, { signal: controller.signal });
        }
      }
    }
    if (controller.signal.aborted) return;
    if (args.rpcError) {
      response = { id: message.id, error: { code: -32602, message: "fake RPC error" } };
    } else if (mode === "error" || args.businessError) {
      response = { id: message.id, result: { content: [{ type: "text", text: "fake tool error" }], isError: true } };
    } else {
      response = { id: message.id, result: { content: [
        { type: "text", text: args.label ? `fixture result: ${args.label}` : "fake capture complete" },
        { type: "image", mimeType: "image/png", data: TINY_PNG_B64 },
      ] } };
    }
    await event("completed", message);
  } catch (error) {
    if (error.name !== "AbortError") throw error;
  } finally {
    clearInterval(ticker);
    pending.delete(key);
    await event("exited", message);
  }
  // The result is the parent's completion signal. Flush lifecycle evidence
  // first, so its temporary directory cannot disappear ahead of these writes.
  if (response) send(response);
}

async function scheduleCatalogExit() {
  if (!exitGateFile && exitAfterCatalogMs <= 0) return;
  if (exitOnceFile) {
    try { const marker = await open(exitOnceFile, "wx"); await marker.close(); }
    catch (error) { if (error.code === "EEXIST") return; throw error; }
  }
  if (exitGateFile) {
    while (true) {
      try { await access(exitGateFile); break; } catch { await delay(10); }
    }
  } else { await delay(exitAfterCatalogMs); }
  await record("process-exit");
  process.exit(0);
}

async function handle(message) {
  if (message.method === "initialize") {
    await record("initialize");
    if (failStartupFile) {
      const fail = await access(failStartupFile).then(() => true, (error) => {
        if (error.code === "ENOENT") return false; throw error;
      });
      if (fail) { await record("startup-exit"); process.exit(0); }
    }
    if (hangStartup) return;
    while (startupGateFile) {
      try { await access(startupGateFile); break; } catch { await delay(10); }
    }
    reply(message.id, {
      protocolVersion: message.params.protocolVersion,
      capabilities: { tools: {} },
      serverInfo: { name: "tomcat-fake-mcp", version: "1.0.0" },
      instructions: "Use fake tools for connector tests.",
    });
  } else if (message.method === "tools/list") {
    await record("tools/list");
    reply(message.id, { tools: [
      { name: "capture", description: "Returns a text result and a tiny PNG",
        inputSchema: { type: "object", properties: {} } },
      { name: "status", description: "Returns fake server status",
        inputSchema: { type: "object", properties: {} } },
    ] });
    await scheduleCatalogExit();
  } else if (message.method === "tools/call") {
    await call(message);
  } else if (message.method === "notifications/cancelled") {
    await event("cancelled", { id: message.params.requestId });
    pending.get(JSON.stringify(message.params.requestId))?.abort();
  } else if (message.method === "ping") {
    reply(message.id, {});
  }
}

// Never await a tool inside the input loop: cancellation and B must pass slow A.
input.on("line", (line) => {
  if (!line.trim()) return;
  try {
    void handle(JSON.parse(line)).catch((error) => {
      console.error(error);
      process.exitCode = 1;
      input.close();
    });
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
    input.close();
  }
});
input.on("close", () => {
  for (const controller of pending.values()) controller.abort();
});
