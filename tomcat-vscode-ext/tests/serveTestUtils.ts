import * as path from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { setupRealServeFixture } from "../scripts/realServeFixture";
import type { LlmApi, ScriptedPart, FixtureSetup } from "../scripts/realServeFixture";
export { serveFixtureEnvironment, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, sseToolCall } from "../scripts/realServeFixture";
export type { LlmApi, ScriptedPart, ScriptedResponse } from "../scripts/realServeFixture";

import { beforeAll, inject } from "vitest";

import { buildTomcatBinary, builtTomcatBinary } from "../scripts/buildTomcatBinary";
import type { ServeEvent } from "../src/serveClient/wire";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";

let buildPromise: Promise<string> | undefined;

export type PlanFileState = "completed" | "executing" | "pending" | "planning";


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
  if (inject("tomcatBuilt")) return builtTomcatBinary();
  // Standalone runs without the integration config still build, rather than trust a stale exe.
  buildPromise ??= buildTomcatBinary().catch((error) => {
    buildPromise = undefined;
    throw error;
  });
  return buildPromise;
}

export function warmTomcatBinaryForSuite(timeoutMs = 120_000): void {
  beforeAll(async () => {
    await ensureTomcatBinary();
  }, timeoutMs);
}

export async function setupServeFixture(
  baseUrl: string,
  api: LlmApi = "openai",
  setup: Partial<FixtureSetup> = {},
) {
  return setupRealServeFixture(baseUrl, api, { ...setup, binary: setup.binary ?? await ensureTomcatBinary() });
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
