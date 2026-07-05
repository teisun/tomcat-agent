import { describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { ParticipantCommands } from "../commands";
import { createParticipantHandler } from "../handler";
import { SessionOwnershipTracker } from "../../webview/ownership";

class CountingMessenger {
  listModelsCalls = 0;
  setPlanModeCalls = 0;

  onEvent() {
    return { dispose() {} };
  }

  async request() {
    return {
      sessionId: "s1",
      success: true,
      type: "response",
    };
  }

  async sendListModels() {
    this.listModelsCalls += 1;
    return {
      payload: { models: [] },
      success: true,
      type: "response",
    };
  }

  async sendSetModel() {
    return {
      success: true,
      type: "response",
    };
  }

  async sendSetPlanMode() {
    this.setPlanModeCalls += 1;
    return {
      success: true,
      type: "response",
    };
  }
}

function createHandler(capabilities: string[]) {
  const messenger = new CountingMessenger();
  const commands = new ParticipantCommands({} as never);
  const stream: string[] = [];
  const handler = createParticipantHandler({
    commands,
    getUiMode: () => "both",
    ide: {} as never,
    initialize: async () => ({
      capabilities,
      protocolVersion: 1,
      sessionId: "s1",
    }),
    messenger: messenger as never,
    ownership: new SessionOwnershipTracker(),
    sessionRouter: {
      buildResultMetadata(sessionId: string) {
        return { sessionId };
      },
      async getState() {
        return {
          busy: false,
          model: "gpt-5.4",
          planId: null,
          planState: "chat",
          sessionId: "s1",
        };
      },
      async resolveSessionId() {
        return "s1";
      },
    } as never,
  });

  return {
    handler,
    messenger,
    stream,
    streamCapture: {
      markdown(value: string) {
        stream.push(value);
      },
      progress() {},
    } as unknown as vscode.ChatResponseStream,
  };
}

function createToken(): vscode.CancellationToken {
  return {
    isCancellationRequested: false,
    onCancellationRequested: () => ({ dispose() {} }),
  } as unknown as vscode.CancellationToken;
}

describe("participant capability gating", () => {
  it("degrades `/plan` when set_plan_mode is unavailable", async () => {
    const harness = createHandler(["prompt", "ask_question"]);

    const result = await harness.handler(
      { command: "plan", prompt: "" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.messenger.setPlanModeCalls).toBe(0);
    expect(harness.stream[0]).toContain("does not support `set_plan_mode`");
  });

  it("degrades `/model` when list_models is unavailable", async () => {
    const harness = createHandler(["prompt", "ask_question", "set_plan_mode"]);

    const result = await harness.handler(
      { command: "model", prompt: "" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.messenger.listModelsCalls).toBe(0);
    expect(harness.stream[0]).toContain("does not support `list_models`");
  });
});
