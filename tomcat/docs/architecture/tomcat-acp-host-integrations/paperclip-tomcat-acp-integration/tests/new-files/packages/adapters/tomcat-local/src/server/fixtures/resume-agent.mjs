import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { createInterface } from "node:readline";

// A controlled ACP endpoint, not a mock of the acpx runtime or adapter.
const trace = process.env.TOMCAT_TEST_TRACE;
const directory = process.argv[process.argv.indexOf("--skills-dir") + 1];
const send = message => process.stdout.write(`${JSON.stringify(message)}\n`);
const record = value => fs.appendFileSync(trace, `${JSON.stringify(value)}\n`);

for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  const request = JSON.parse(line);
  const { method, params = {} } = request;
  record({ method, sessionId: params.sessionId, directory, mcpServers: params.mcpServers });
  let result;
  if (method === "initialize") result = {
    protocolVersion: 1,
    agentCapabilities: { loadSession: true, mcpCapabilities: { http: true } },
    agentInfo: { name: "controlled-tomcat", version: "1.0.0" },
  };
  else if (method === "session/new") result = { sessionId: randomUUID() };
  else if (method === "session/load") result = {};
  else if (method === "session/prompt") {
    const body = fs.readFileSync(path.join(directory, "work/SKILL.md"), "utf8");
    record({ method: "skill-read", directory, body });
    send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: params.sessionId, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Completed with fresh skills." } } } });
    result = { stopReason: "end_turn" };
  } else if (request.id !== undefined) {
    send({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "Method not supported" } });
    continue;
  }
  if (request.id !== undefined) send({ jsonrpc: "2.0", id: request.id, result });
}
