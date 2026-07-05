import { describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { createParticipantHandler } from "../src/ui/participant/handler";
import { ParticipantCommands } from "../src/ui/participant/commands";
import { SessionOwnershipTracker } from "../src/ui/webview/ownership";

describe("session ownership", () => {
  it("blocks participant turns when the webview owns the same session", async () => {
    const ownership = new SessionOwnershipTracker();
    ownership.claim("s1", "webview");

    const handler = createParticipantHandler({
      commands: new ParticipantCommands({} as never),
      getUiMode: () => "both",
      ide: {} as never,
      initialize: async () => ({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
        sessionId: "s1",
      }),
      messenger: {
        onEvent() {
          return { dispose() {} };
        },
        async request() {
          return {
            payload: { accepted: true },
            sessionId: "s1",
            success: true,
            type: "response",
          };
        },
      } as never,
      ownership,
      sessionRouter: {
        async resolveSessionId() {
          return "s1";
        },
      } as never,
    });

    const result = await handler(
      { prompt: "hello" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      {
        markdown() {},
        progress() {},
      } as unknown as vscode.ChatResponseStream,
      {
        isCancellationRequested: false,
        onCancellationRequested: () => ({ dispose() {} }),
      } as unknown as vscode.CancellationToken,
    );

    expect(result?.errorDetails?.message).toContain("owned by the Tomcat webview");
    expect(ownership.release("s1", "participant")).toBe(false);
    expect(ownership.release("s1", "webview")).toBe(true);
    expect(ownership.claim("s1", "participant").ok).toBe(true);
  });

  it("releases participant ownership after a completed turn", async () => {
    const ownership = new SessionOwnershipTracker();
    const listeners = new Set<(event: { sessionId: string; type: string }) => void>();

    const handler = createParticipantHandler({
      commands: {
        attachTurn() {
          return { dispose() {} };
        },
      } as never,
      getUiMode: () => "both",
      ide: {} as never,
      initialize: async () => ({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
        sessionId: "s1",
      }),
      messenger: {
        onEvent(listener: (event: { sessionId: string; type: string }) => void) {
          listeners.add(listener);
          return {
            dispose() {
              listeners.delete(listener);
            },
          };
        },
        async request() {
          for (const listener of listeners) {
            listener({ sessionId: "s1", type: "agent_end" });
          }
          return {
            payload: { accepted: true },
            sessionId: "s1",
            success: true,
            type: "response",
          };
        },
      } as never,
      ownership,
      sessionRouter: {
        buildResultMetadata(sessionId: string) {
          return { sessionId };
        },
        async getState() {
          return null;
        },
        async resolveSessionId() {
          return "s1";
        },
      } as never,
    });

    const result = await handler(
      { prompt: "hello" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      {
        markdown() {},
        progress() {},
      } as unknown as vscode.ChatResponseStream,
      {
        isCancellationRequested: false,
        onCancellationRequested: () => ({ dispose() {} }),
      } as unknown as vscode.CancellationToken,
    );

    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(ownership.ownerOf("s1")).toBeUndefined();
    expect(ownership.claim("s1", "webview").ok).toBe(true);
  });
});
