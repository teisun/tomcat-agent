import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import { SessionRouter } from "../src/serveClient/sessionRouter";
import type { WireEvent } from "../src/serveClient/wire";
import {
  createRealServeMessenger,
  spawnScriptedOpenAiStreamServer,
  sseDelta,
  sseDone,
  sseFinish,
} from "./serveTestUtils";

function sessionOf(event: WireEvent): string | undefined {
  return (event as WireEvent & { sessionId?: string }).sessionId;
}

async function collectUntil(
  messenger: { onEvent(listener: (event: WireEvent) => void): { dispose(): void } },
  predicate: (events: WireEvent[]) => boolean,
  timeoutMs = 10_000,
): Promise<WireEvent[]> {
  return new Promise((resolve, reject) => {
    const events: WireEvent[] = [];
    const timer = setTimeout(() => {
      disposable.dispose();
      reject(new Error("timed out waiting for session lifecycle events"));
    }, timeoutMs);
    const disposable = messenger.onEvent((event) => {
      events.push(event);
      if (predicate(events)) {
        clearTimeout(timer);
        disposable.dispose();
        resolve(events);
      }
    });
  });
}

describe("session lifecycle integration", () => {
  it("keeps concurrent sessions isolated", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [
          sseDelta("slow"),
          { body: sseFinish("stop").body, delayMs: 250 },
          sseDone(),
        ],
      },
      {
        parts: [sseDelta("fast"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const sessionRouter = new SessionRouter(
        runtime.messenger,
        () => runtime.fixture.workspacePath,
      );
      sessionRouter.setBootstrapSessionId(init.sessionId);
      const sessionA = init.sessionId!;
      const sessionB = await sessionRouter.newSession();

      const list = await sessionRouter.listSessions();
      expect(list.sessions.map((session) => session.sessionId)).toEqual(
        expect.arrayContaining([sessionA, sessionB]),
      );

      const allDone = collectUntil(
        runtime.messenger,
        (events) =>
          events.filter((event) => event.type === "agent_end" && sessionOf(event) === sessionA)
            .length > 0 &&
          events.filter((event) => event.type === "agent_end" && sessionOf(event) === sessionB)
            .length > 0,
      );

      await Promise.all([
        runtime.messenger.request({
          params: {},
          sessionId: sessionA,
          text: "slow",
          type: "prompt",
        }),
        runtime.messenger.request({
          params: {},
          sessionId: sessionB,
          text: "fast",
          type: "prompt",
        }),
      ]);

      const events = await allDone;
      const firstEnd = events.findIndex((event) => event.type === "agent_end");
      const startA = events.findIndex(
        (event) => event.type === "agent_start" && sessionOf(event) === sessionA,
      );
      const startB = events.findIndex(
        (event) => event.type === "agent_start" && sessionOf(event) === sessionB,
      );

      expect(startA).toBeGreaterThanOrEqual(0);
      expect(startB).toBeGreaterThanOrEqual(0);
      expect(Math.max(startA, startB)).toBeLessThan(firstEnd);
      const deltasBySession = new Map<string, string[]>();
      for (const event of events) {
        if (event.type !== "message_update") {
          continue;
        }
        const sessionId = sessionOf(event);
        const delta = (event.assistantMessageEvent as { delta?: string }).delta;
        if (!sessionId || !delta) {
          continue;
        }
        deltasBySession.set(sessionId, [
          ...(deltasBySession.get(sessionId) ?? []),
          delta,
        ]);
      }

      expect(deltasBySession.has(sessionA)).toBe(true);
      expect(deltasBySession.has(sessionB)).toBe(true);
      const flattened = [...deltasBySession.values()].flat();
      expect(flattened).toEqual(expect.arrayContaining(["slow", "fast"]));
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);

  it("interrupts an active session and emits the terminal lifecycle events", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [
          sseDelta("partial"),
          { body: sseFinish("stop").body, delayMs: 350 },
          sseDone(),
        ],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      const sessionId = init.sessionId!;

      const firstDelta = collectUntil(
        runtime.messenger,
        (events) =>
          events.some(
            (event) =>
              event.type === "message_update" &&
              sessionOf(event) === sessionId &&
              (event.assistantMessageEvent as { delta?: string }).delta === "partial",
          ),
      );

      await runtime.messenger.request({
        params: {},
        sessionId,
        text: "start then interrupt",
        type: "prompt",
      });
      await firstDelta;

      const endEvents = collectUntil(
        runtime.messenger,
        (events) =>
          events.some(
            (event) => event.type === "agent_end" && sessionOf(event) === sessionId,
          ),
      );
      const interruptResponse = await runtime.messenger.request({
        sessionId,
        type: "interrupt",
      });
      const events = await endEvents;

      expect(interruptResponse.success).toBe(true);
      expect(
        events.some(
          (event) => event.type === "agent_interrupted" && sessionOf(event) === sessionId,
        ),
      ).toBe(true);
      expect(
        events.some(
          (event) =>
            event.type === "agent_end" &&
            sessionOf(event) === sessionId &&
            event.error === "interrupted",
        ),
      ).toBe(true);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);

  it("can restart after the serve child exits unexpectedly", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [sseDelta("after restart"), sseFinish("stop"), sseDone()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl);

    try {
      const init = await initializeServe(runtime.messenger);
      expect(init.sessionId).toBeTruthy();

      const exited = new Promise<void>((resolve) => {
        runtime.messenger.onExit(() => resolve());
      });
      process.kill(runtime.messenger.pid!, "SIGTERM");
      await exited;

      const reinit = await initializeServe(runtime.messenger);
      const sessionId = reinit.sessionId!;
      const endEvents = collectUntil(
        runtime.messenger,
        (events) =>
          events.some(
            (event) => event.type === "agent_end" && sessionOf(event) === sessionId,
          ),
      );

      await runtime.messenger.request({
        params: {},
        sessionId,
        text: "say hello after restart",
        type: "prompt",
      });
      const events = await endEvents;

      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            sessionOf(event) === sessionId &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "after restart",
        ),
      ).toBe(true);
    } finally {
      await runtime.cleanup();
      await server.close();
    }
  }, 30_000);
});
