import * as os from "node:os";
import * as path from "node:path";
import * as http from "node:http";
import { setTimeout as delay } from "node:timers/promises";
import type { Socket } from "node:net";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";

import { beforeAll } from "vitest";

import { resolveCargoCommand } from "../scripts/resolveCargoCommand";
import type { ServeEvent } from "../src/serveClient/wire";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";

const execFileAsync = promisify(execFile);
const repoRoot = path.resolve(__dirname, "..", "..");
const tomcatRoot = path.resolve(repoRoot, "tomcat");
const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.resolve(tomcatRoot, "target");
const tomcatBinary = path.resolve(
  cargoTargetDir,
  "debug",
  process.platform === "win32" ? "tomcat.exe" : "tomcat",
);

let buildPromise: Promise<void> | undefined;

export type ScriptedPart = {
  body: string;
  delayMs?: number;
};

export type ScriptedResponse = {
  parts: ScriptedPart[];
};

export type LlmApi = "openai" | "openai-responses";
export type PlanFileState = "completed" | "executing" | "pending" | "planning";

export function sseDelta(content: string): ScriptedPart {
  return {
    body: `data: {"choices":[{"delta":{"content":"${content}"}}]}\n\n`,
  };
}

export function sseFinish(reason: string): ScriptedPart {
  return {
    body: `data: ${JSON.stringify({ choices: [{ finish_reason: reason }] })}\n\n`,
  };
}

export function sseDone(): ScriptedPart {
  return { body: "data: [DONE]\n\n" };
}

export function sseToolCall(id: string, name: string, argsJson: string): ScriptedPart {
  const serializedArgs = JSON.stringify(argsJson);
  return {
    body:
      `data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"${id}",` +
      `"function":{"name":"${name}","arguments":${serializedArgs}}}]}}]}\n\n`,
  };
}

export function responsesFunctionCallAdded(
  itemId: string,
  callId: string,
  name: string,
): ScriptedPart {
  return {
    body:
      `data: {"type":"response.output_item.added","item":{"type":"function_call",` +
      `"id":"${itemId}","call_id":"${callId}","name":"${name}","arguments":""}}\n\n`,
  };
}

export function responsesTextDelta(content: string): ScriptedPart {
  return {
    body:
      `data: {"type":"response.output_text.delta","item_id":"m1","content_index":0,` +
      `"delta":${JSON.stringify(content)}}\n\n`,
  };
}

export function responsesFunctionCallArgumentsDelta(
  itemId: string,
  delta: string,
): ScriptedPart {
  return {
    body:
      `data: {"type":"response.function_call_arguments.delta","item_id":"${itemId}",` +
      `"delta":${JSON.stringify(delta)}}\n\n`,
  };
}

export function responsesCompleted(status = "completed"): ScriptedPart {
  return {
    body:
      `data: {"type":"response.completed","response":{"status":"${status}",` +
      `"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}\n\n`,
  };
}

export async function ensureTomcatBinary(): Promise<string> {
  buildPromise ??= execFileAsync(resolveCargoCommand(), ["build", "--quiet", "--bin", "tomcat"], {
    cwd: tomcatRoot,
    timeout: 120_000,
    killSignal: "SIGKILL",
  }).then(() => undefined).catch((error) => {
    buildPromise = undefined;
    throw error;
  });
  await buildPromise;
  if (!(await stat(tomcatBinary).then(() => true).catch(() => false))) {
    throw new Error(`tomcat binary not found after build: ${tomcatBinary}`);
  }
  return tomcatBinary;
}

export function warmTomcatBinaryForSuite(timeoutMs = 120_000): void {
  beforeAll(async () => {
    await ensureTomcatBinary();
  }, timeoutMs);
}

export function serveFixtureEnvironment(homePath: string, inherited: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env = { ...inherited };
  // The fixture owns a disposable HOME and storage directory, so child `tomcat init`
  // commands cannot touch the agent session. Override the inherited nested-agent
  // guard for that private subprocess or the real-serve test cannot initialize.
  env.TOMCAT_AGENT_ACTIVE = undefined;
  for (const key of Object.keys(env)) {
    if (key.startsWith("TOMCAT__") || /_API_KEY$/i.test(key)) env[key] = undefined;
  }
  return {
    ...env,
    HOME: homePath,
    USERPROFILE: homePath,
    SHELL: "/bin/zsh",
    TOMCAT__STORAGE__WORK_DIR: path.join(homePath, ".tomcat"),
    TOMCAT__CONTEXT__COMPACTION_MODEL: "gpt-5.4",
    TOMCAT__LLM__DEFAULT_MODEL: "gpt-5.4",
    OPENAI_API_KEY: "dummy-key",
    ALL_PROXY: "", HTTPS_PROXY: "", HTTP_PROXY: "",
    all_proxy: "", https_proxy: "", http_proxy: "",
    NO_PROXY: "127.0.0.1,localhost", no_proxy: "127.0.0.1,localhost",
  };
}

type FixtureSetup = {
  binary?: string;
  initialize?: (binary: string, env: NodeJS.ProcessEnv, cwd: string) => Promise<void>;
};

export async function setupServeFixture(
  baseUrl: string,
  api: LlmApi = "openai",
  setup: FixtureSetup = {},
): Promise<{
  cleanup(): Promise<void>;
  env: NodeJS.ProcessEnv;
  homePath: string;
  workspacePath: string;
}> {
  const binary = setup.binary ?? await ensureTomcatBinary();
  const homePath = await mkdtemp(path.join(os.tmpdir(), "tomcat-vscode-ext-"));
  const workspacePath = path.join(homePath, "workspace");
  const env = serveFixtureEnvironment(homePath);
  const cleanup = () => rm(homePath, { force: true, recursive: true });
  try {
    await mkdir(workspacePath, { recursive: true });
    if (setup.initialize) {
      await setup.initialize(binary, env, workspacePath);
    } else {
      await execFileAsync(binary, ["init"], {
        cwd: workspacePath, env, timeout: 15_000, killSignal: "SIGKILL",
      });
    }
    const modelsPath = path.join(homePath, ".tomcat", "models.toml");
    // This suite does not exercise connectors. Init installs a live Playwright
    // default; explicitly replace it inside this fixture before starting serve.
    await writeFile(path.join(homePath, ".tomcat", "mcp.json"), '{"mcpServers":{}}\n', "utf8");
    await writeFile(modelsPath, `[[models]]
id = "gpt-5.4"
api = "${api}"
provider = "openai"
base_url = "${baseUrl}"
capabilities = { vision = false, files = false, tools = true, reasoning = true, web_search = false }
`, "utf8");
    return { cleanup, env, homePath, workspacePath };
  } catch (error) {
    try { await cleanup(); } catch (cleanupError) {
      console.warn("serve fixture cleanup failed after setup error", cleanupError);
    }
    throw error;
  }
}

export async function spawnScriptedOpenAiStreamServer(responses: ScriptedResponse[]): Promise<{
  baseUrl: string;
  capturedRequests(): string[];
  capturedNonTitleRequests(): string[];
  close(): Promise<void>;
}> {
  const captured: string[] = [];
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
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });

  async function respond(request: http.IncomingMessage, response: http.ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of request) {
      chunks.push(Buffer.from(chunk));
    }
    const body = Buffer.concat(chunks).toString("utf8");
    const headers = Object.entries(request.headers)
      .map(([key, value]) => `${key}: ${Array.isArray(value) ? value.join(",") : value ?? ""}`)
      .join("\r\n");
    const rawRequest = `${request.method} ${request.url} HTTP/1.1\r\n${headers}\r\n\r\n${body}`;
    captured.push(rawRequest);

    if (isSessionTitleRequest(rawRequest)) {
      response.writeHead(200, {
        Connection: "close",
        "Content-Type": "application/json",
      });
      response.end(sessionTitleResponseJson(rawRequest, "Generated title"));
      return;
    }

    const scripted = responses[responseIndex++];
    if (!scripted) {
      response.statusCode = 500;
      response.end("unexpected request");
      return;
    }

    response.writeHead(200, {
      "Cache-Control": "no-cache",
      Connection: "close",
      "Content-Type": "text/event-stream",
    });

    for (const part of scripted.parts) {
      if (part.delayMs && part.delayMs > 0) {
        await delay(part.delayMs, undefined, { signal: abort.signal });
      }
      response.write(part.body);
    }
    response.end();
  }

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve();
    });
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("failed to bind scripted OpenAI server");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    capturedRequests() {
      return [...captured];
    },
    capturedNonTitleRequests() {
      return captured.filter((request) => !isSessionTitleRequest(request));
    },
    close() {
      if (closePromise) return closePromise;
      abort.abort();
      closePromise = (async () => {
        let timer: NodeJS.Timeout | undefined;
        try {
          await Promise.race([
            new Promise<void>((resolve, reject) => {
              server.close((error) => error ? reject(error) : resolve());
              for (const socket of sockets) socket.destroy();
            }).then(() => Promise.all([...inFlight])),
            new Promise<never>((_resolve, reject) => {
              timer = setTimeout(() => reject(new Error("scripted server cleanup timed out")), 3_000);
            }),
          ]);
        } finally { clearTimeout(timer); }
      })();
      return closePromise;
    },
  };
}

function isSessionTitleRequest(rawRequest: string): boolean {
  const body = rawRequest.split("\r\n\r\n")[1];
  if (!body) {
    return false;
  }

  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return false;
  }

  if (!isRecord(value)) {
    return false;
  }
  return containsSessionTitlePrompt(value);
}

function containsSessionTitlePrompt(value: unknown): boolean {
  if (typeof value === "string") {
    return value.includes("Generate a short chat title from the user's first message.");
  }
  if (Array.isArray(value)) {
    return value.some(containsSessionTitlePrompt);
  }
  return isRecord(value) && Object.values(value).some(containsSessionTitlePrompt);
}

function sessionTitleResponseJson(rawRequest: string, title: string): string {
  if (rawRequest.startsWith("POST /v1/responses ")) {
    return JSON.stringify({
      id: "title-mock",
      output: [
        {
          content: [
            {
              text: title,
              type: "output_text",
            },
          ],
          type: "message",
        },
      ],
      status: "completed",
      usage: {
        input_tokens: 1,
        output_tokens: 1,
        total_tokens: 2,
      },
    });
  }

  return JSON.stringify({
    choices: [
      {
        finish_reason: "stop",
        index: 0,
        message: {
          content: title,
          role: "assistant",
        },
      },
    ],
    id: "title-mock",
    usage: {
      completion_tokens: 1,
      prompt_tokens: 1,
      total_tokens: 2,
    },
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export async function createRealServeMessenger(
  baseUrl: string,
  api: LlmApi = "openai",
): Promise<{
  cleanup(): Promise<void>;
  fixture: Awaited<ReturnType<typeof setupServeFixture>>;
  messenger: TomcatMessenger;
}> {
  const fixture = await setupServeFixture(baseUrl, api);
  let messenger: TomcatMessenger;
  try {
    messenger = new TomcatMessenger({
      cwd: fixture.workspacePath,
      env: fixture.env,
      executable: await ensureTomcatBinary(),
      requestTimeoutMs: 10000,
    });
  } catch (error) {
    try { await fixture.cleanup(); } catch (cleanupError) {
      console.warn("serve fixture cleanup failed after construction error", cleanupError);
    }
    throw error;
  }

  return {
    async cleanup() {
      await messenger.disposeAsync();
      await fixture.cleanup();
    },
    fixture,
    messenger,
  };
}

export async function waitForEvent(
  messenger: TomcatMessenger,
  predicate: (event: ServeEvent) => boolean,
  timeoutMs = 10000,
): Promise<ServeEvent[]> {
  return new Promise((resolve, reject) => {
    const seen: ServeEvent[] = [];
    const cleanup = () => {
      clearTimeout(timer);
      stderrDisposable.dispose();
      disposable.dispose();
    };
    const timer = setTimeout(() => {
      const seenTypes = seen.map((event) => event.type).join(", ") || "(none)";
      const stderr = messenger.recentStderr.trim() || "(empty)";
      cleanup();
      reject(
        new Error(
          `timed out waiting for matching event; seen=${seenTypes}; stderr=${stderr}`,
        ),
      );
    }, timeoutMs);
    const stderrDisposable = messenger.onStderr(() => {});
    const disposable = messenger.onEvent((event) => {
      seen.push(event);
      if (predicate(event)) {
        cleanup();
        resolve(seen);
      }
    });
  });
}

// State/event tests exercise build acknowledgement, not the execution/Acceptance
// loop. Wait for real streamed text, then use the public interrupt command before
// the scripted reply finishes. All waits join immediately, so failures stay owned.
export async function buildAndInterruptPlan(
  messenger: TomcatMessenger, sessionId: string, planPath: string,
) {
  const events = waitForEvent(messenger, (event) => event.type === "agent_idle" && event.sessionId === sessionId);
  const interrupted = waitForEvent(messenger, (event) => event.type === "message_update" && event.sessionId === sessionId)
    .then(async () => {
      const runningState = await messenger.request({ type: "get_state", sessionId });
      const response = await messenger.request({ type: "interrupt", sessionId });
      return { runningState, response };
    });
  const [build, captured, interrupt] = await Promise.all([
    messenger.sendSetPlanMode({ action: "build", planId: planPath, sessionId }), events, interrupted,
  ]);
  if (!interrupt.response.success) throw new Error(`fixture interrupt failed: ${JSON.stringify(interrupt.response)}`);
  return { build, events: captured, runningState: interrupt.runningState };
}

export async function readRequestJson(rawRequest: string): Promise<unknown> {
  const body = rawRequest.split("\r\n\r\n")[1] ?? "";
  return JSON.parse(body);
}

export async function readConfigText(homePath: string): Promise<string> {
  return readFile(path.join(homePath, ".tomcat", "tomcat.config.toml"), "utf8");
}

export async function writePlanFile(
  homePath: string,
  planId: string,
  state: PlanFileState = "planning",
): Promise<string> {
  const plansDir = path.join(homePath, ".tomcat", "plans");
  await mkdir(plansDir, { recursive: true });
  const planPath = path.join(plansDir, `${planId}.plan.md`);
  await writeFile(
    planPath,
    `---
plan_id: ${planId}
goal: Stage A integration plan
state: ${state}
session_key: null
session_id: null
created_at: ${new Date().toISOString()}
schema_version: 1
todos:
- id: step1
  content: Do the thing
  status: pending
---
## Goal

Stage A integration plan

## Plan

1. Do the thing.

## Todos Board

<!-- todos-board:auto:begin -->
### Todos
- [ ] step1: Do the thing
<!-- todos-board:auto:end -->
`,
    "utf8",
  );
  return planPath;
}
