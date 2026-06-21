import { describe, expect, it } from "vitest";
import * as vscode from "vscode";

import type { InitializeResult } from "../src/serveClient/initialize";
import { createParticipantHandler } from "../src/ui/participant/handler";
import { ParticipantCommands } from "../src/ui/participant/commands";
import { SessionOwnershipTracker } from "../src/ui/webview/ownership";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";

class SharedMessenger {
  readonly requestCalls: Array<Record<string, unknown>> = [];
  private readonly listeners = new Set<(event: Record<string, unknown>) => void>();

  onEvent(listener: (event: Record<string, unknown>) => void) {
    this.listeners.add(listener);
    return { dispose() {} };
  }

  async request(command: Record<string, unknown>) {
    this.requestCalls.push(command);
    if (command.type === "prompt" || command.type === "follow_up") {
      queueMicrotask(() => {
        for (const listener of this.listeners) {
          listener({
            error: null,
            messages: [],
            sessionId: String(command.sessionId ?? "chat-1"),
            type: "agent_end",
          });
        }
      });
    }
    return {
      payload: { accepted: true },
      sessionId: String(command.sessionId ?? "chat-1"),
      success: true,
      type: "response",
    };
  }

  async sendListModels() {
    return {
      payload: { models: [{ id: "gpt-5.4" }] },
      success: true,
      type: "response",
    };
  }

  async sendSetModel() {
    return { success: true, type: "response" };
  }

  async sendSetPlanMode() {
    return {
      payload: { planState: "planning" },
      success: true,
      type: "response",
    };
  }
}

function initializeResult(): InitializeResult {
  return {
    capabilities: ["prompt", "ask_question", "list_models", "set_plan_mode"],
    protocolVersion: 1,
    sessionId: "chat-1",
  };
}

describe("dual frontend bridge reuse", () => {
  it("uses the same messenger instance for the participant and the webview", async () => {
    const messenger = new SharedMessenger();
    const ownership = new SessionOwnershipTracker();
    const commands = new ParticipantCommands({} as never);
    const sessionState = new Map<string, string | null>([
      ["chat-1", null],
      ["web-1", null],
    ]);
    const sessionRouter = {
      buildResultMetadata(sessionId: string) {
        return { sessionId };
      },
      async getState(sessionId?: string) {
        return {
          busy: false,
          model: "gpt-5.4",
          planId: null,
          planState: sessionState.get(sessionId ?? "chat-1") ?? "chat",
          sessionId: sessionId ?? "chat-1",
        };
      },
      async listSessions() {
        return {
          activeSessionId: "web-1",
          scope: "disk" as const,
          sessions: [
            { busy: false, isCurrent: true, sessionId: "web-1", updatedAt: 1 },
          ],
        };
      },
      async newSession() {
        return "web-1";
      },
      async resolveSessionId() {
        return "chat-1";
      },
      async switchSession(sessionId: string) {
        return sessionId;
      },
    };

    const participant = createParticipantHandler({
      commands,
      getUiMode: () => "both",
      ide: {} as never,
      initialize: async () => initializeResult(),
      messenger: messenger as never,
      ownership,
      sessionRouter: sessionRouter as never,
    });
    const webview = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "both",
      ide: {
        applyPreparedEdit: async () => true,
        openPreparedDiff: async () => undefined,
        rememberToolResult: async () => ({
          displayPath: "src/app.ts",
          originalContent: "",
          proposedContent: "",
          toolCallId: "tool-1",
        }),
        rememberToolStart: async () => undefined,
      } as never,
      initialize: async () => initializeResult(),
      messenger: messenger as never,
      ownership,
      sessionRouter: sessionRouter as never,
    });

    await participant(
      { prompt: "hello participant" } as vscode.ChatRequest,
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

    await webview.dispatchTestIntent({
      data: { sessionId: "web-1", text: "hello webview" },
      messageId: "prompt-web-1",
      type: "prompt",
    });

    expect(messenger.requestCalls).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ text: "hello participant", type: "prompt" }),
        expect.objectContaining({ text: "hello webview", type: "prompt" }),
      ]),
    );
    expect(messenger.requestCalls).toHaveLength(2);
    webview.dispose();
  });
});
