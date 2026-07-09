import * as os from "node:os";
import * as path from "node:path";
import * as http from "node:http";
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
    body: `data: {"choices":[{"finish_reason":"${reason}"}}]}\n\n`,
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
  }).then(() => undefined);
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

export async function setupServeFixture(
  baseUrl: string,
  api: LlmApi = "openai",
): Promise<{
  cleanup(): Promise<void>;
  env: NodeJS.ProcessEnv;
  homePath: string;
  workspacePath: string;
}> {
  const binary = await ensureTomcatBinary();
  const homePath = await mkdtemp(path.join(os.tmpdir(), "tomcat-vscode-ext-"));
  const workspacePath = path.join(homePath, "workspace");
  await mkdir(workspacePath, { recursive: true });

  await execFileAsync(binary, ["init"], {
    env: {
      ...process.env,
      HOME: homePath,
      SHELL: "/bin/zsh",
    },
  });

  const modelsPath = path.join(homePath, ".tomcat", "models.toml");
  await writeFile(
    modelsPath,
    `[[models]]
id = "gpt-5.4"
api = "${api}"
provider = "openai"
base_url = "${baseUrl}"
capabilities = { vision = false, files = false, tools = true, reasoning = true, web_search = false }
`,
    "utf8",
  );

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    ALL_PROXY: "",
    HOME: homePath,
    HTTPS_PROXY: "",
    HTTP_PROXY: "",
    NO_PROXY: "127.0.0.1,localhost",
    OPENAI_API_KEY: "dummy-key",
    SHELL: "/bin/zsh",
    TOMCAT__CONTEXT__COMPACTION_MODEL: "gpt-5.4",
    TOMCAT__LLM__DEFAULT_MODEL: "gpt-5.4",
    all_proxy: "",
    https_proxy: "",
    http_proxy: "",
    no_proxy: "127.0.0.1,localhost",
  };

  return {
    async cleanup() {
      await rm(homePath, { force: true, recursive: true });
    },
    env,
    homePath,
    workspacePath,
  };
}

export async function spawnScriptedOpenAiStreamServer(responses: ScriptedResponse[]): Promise<{
  baseUrl: string;
  capturedRequests(): string[];
  capturedNonTitleRequests(): string[];
  close(): Promise<void>;
}> {
  const captured: string[] = [];
  let responseIndex = 0;

  const server = http.createServer(async (request, response) => {
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
        await new Promise((resolve) => setTimeout(resolve, part.delayMs));
      }
      response.write(part.body);
    }
    response.end();
  });

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
    async close() {
      await new Promise<void>((resolve, reject) => {
        server.close((error) => {
          if (error) {
            reject(error);
            return;
          }
          resolve();
        });
      });
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

  if (!isRecord(value) || value.stream !== false) {
    return false;
  }
  return body.includes("Generate a short chat title from the user's first message.\\n");
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
  const messenger = new TomcatMessenger({
    cwd: fixture.workspacePath,
    env: fixture.env,
    executable: await ensureTomcatBinary(),
    requestTimeoutMs: 10000,
  });

  return {
    async cleanup() {
      messenger.dispose();
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
