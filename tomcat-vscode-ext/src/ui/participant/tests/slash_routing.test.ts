import { beforeEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { createParticipantHandler } from "../handler";
import { ParticipantCommands } from "../commands";
import type { ServeEvent } from "../../../serveClient/wire";
import { SessionOwnershipTracker } from "../../webview/ownership";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      reset(): void;
      setQuickPickHandler(
        handler: (
          items: Array<{ description?: string; label: string }>,
        ) => { label: string } | undefined,
      ): void;
    };
  }
).__testing;

class FakeMessenger {
  currentModel = "gpt-5.4";
  currentPlanId: string | null = null;
  currentPlanState = "chat";
  readonly listModelCalls: string[] = [];
  readonly requestCalls: Array<Record<string, unknown>> = [];
  readonly setModelCalls: Array<{ model: string; sessionId?: string | null }> = [];
  readonly setPlanModeCalls: Array<{
    action: string;
    planId?: string | null;
    sessionId?: string | null;
  }> = [];
  private readonly eventListeners = new Set<(event: ServeEvent) => void>();

  onEvent(listener: (event: ServeEvent) => void) {
    this.eventListeners.add(listener);
    return {
      dispose: () => {
        this.eventListeners.delete(listener);
      },
    };
  }

  async request(command: Record<string, unknown>) {
    this.requestCalls.push(command);
    return {
      sessionId: "s1",
      success: true,
      type: "response",
    };
  }

  async sendListModels() {
    this.listModelCalls.push("list_models");
    return {
      payload: {
        models: [
          { id: "gpt-5.4", provider: "openai" },
          { id: "claude-opus-4", provider: "anthropic" },
        ],
      },
      success: true,
      type: "response",
    };
  }

  async sendSetModel(sessionId: string | null | undefined, model: string) {
    this.currentModel = model;
    this.setModelCalls.push({ model, sessionId });
    return {
      sessionId,
      success: true,
      type: "response",
    };
  }

  async sendSetPlanMode(command: {
    action: "build" | "enter" | "exit";
    planId?: string | null;
    sessionId?: string | null;
  }) {
    this.setPlanModeCalls.push(command);

    if (command.action === "enter") {
      this.currentPlanId = null;
      this.currentPlanState = "planning";
      return {
        payload: { planState: "planning" },
        sessionId: command.sessionId,
        success: true,
        type: "response",
      };
    }

    if (command.action === "exit") {
      this.currentPlanId = null;
      this.currentPlanState = "chat";
      return {
        payload: { planState: "chat" },
        sessionId: command.sessionId,
        success: true,
        type: "response",
      };
    }

    this.currentPlanId = command.planId ?? "default-plan";
    this.currentPlanState = "executing";
    queueMicrotask(() => {
      this.emitEvent({
        sessionId: command.sessionId,
        type: "agent_start",
      });
      queueMicrotask(() => {
        this.emitEvent({
          error: null,
          messages: [],
          sessionId: command.sessionId,
          type: "agent_end",
        });
      });
    });
    return {
      payload: {
        planId: this.currentPlanId,
        planPath: `/tmp/${this.currentPlanId}.plan.md`,
        planState: "executing",
      },
      sessionId: command.sessionId,
      success: true,
      type: "response",
    };
  }

  private emitEvent(event: ServeEvent): void {
    for (const listener of this.eventListeners) {
      listener(event);
    }
  }
}

function createHandlerHarness(
  capabilities = [
    "prompt",
    "ask_question",
    "list_models",
    "set_plan_mode",
  ],
) {
  const messenger = new FakeMessenger();
  const commands = new ParticipantCommands({} as never);
  const stream: Array<{ kind: "markdown" | "progress"; value: string }> = [];
  const sessionRouter = {
    async getState() {
      return {
        busy: false,
        model: messenger.currentModel,
        planId: messenger.currentPlanId,
        planState: messenger.currentPlanState,
        sessionId: "s1",
        sessionKey: "scope-key",
      };
    },
    buildResultMetadata(sessionId: string) {
      return { sessionId };
    },
    async resolveSessionId() {
      return "s1";
    },
  };

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
    sessionRouter: sessionRouter as never,
  });

  return {
    handler,
    messenger,
    stream,
    streamCapture: {
      markdown(value: string) {
        stream.push({ kind: "markdown", value });
      },
      progress(value: string) {
        stream.push({ kind: "progress", value });
      },
    } as vscode.ChatResponseStream,
  };
}

function createToken(): vscode.CancellationToken {
  return {
    isCancellationRequested: false,
    onCancellationRequested: () => ({ dispose() {} }),
  } as unknown as vscode.CancellationToken;
}

describe("participant slash routing", () => {
  beforeEach(() => {
    __testing.reset();
  });

  it("routes `/plan` to enter plan mode", async () => {
    const harness = createHandlerHarness();

    const result = await harness.handler(
      { command: "plan", prompt: "" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(harness.messenger.setPlanModeCalls).toEqual([
      { action: "enter", sessionId: "s1" },
    ]);
    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.stream).toEqual(
      expect.arrayContaining([
        { kind: "progress", value: "Tomcat plan mode" },
        { kind: "markdown", value: "Tomcat entered plan mode." },
      ]),
    );
  });

  it("routes `/plan exit` to exit plan mode", async () => {
    const harness = createHandlerHarness();

    const result = await harness.handler(
      { command: "plan", prompt: "exit" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(harness.messenger.setPlanModeCalls).toEqual([
      { action: "exit", sessionId: "s1" },
    ]);
    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.stream).toEqual(
      expect.arrayContaining([
        { kind: "progress", value: "Tomcat chat mode" },
        { kind: "markdown", value: "Tomcat exited plan mode." },
      ]),
    );
  });

  it("routes `/plan build` to build mode and waits for agent end", async () => {
    const harness = createHandlerHarness();

    const result = await harness.handler(
      { command: "plan", prompt: "build plan-42" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(harness.messenger.setPlanModeCalls).toEqual([
      { action: "build", planId: "plan-42", sessionId: "s1" },
    ]);
    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.stream).toEqual(
      expect.arrayContaining([
        { kind: "progress", value: "Tomcat executing plan (plan-42)" },
        {
          kind: "markdown",
          value: "Started building `/tmp/plan-42.plan.md`.",
        },
        { kind: "progress", value: "Tomcat agent started" },
      ]),
    );
  });

  it("routes `/model` through list, quick pick, and set_model", async () => {
    __testing.setQuickPickHandler(
      (items: Array<{ label: string }>) =>
        items.find((item) => item.label === "claude-opus-4"),
    );
    const harness = createHandlerHarness();

    const result = await harness.handler(
      { command: "model", prompt: "" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(harness.messenger.listModelCalls).toEqual(["list_models"]);
    expect(harness.messenger.setModelCalls).toEqual([
      { model: "claude-opus-4", sessionId: "s1" },
    ]);
    expect(result?.metadata).toEqual({ sessionId: "s1" });
    expect(harness.stream).toEqual(
      expect.arrayContaining([
        {
          kind: "markdown",
          value: "Switched Tomcat model to `claude-opus-4`.",
        },
      ]),
    );
  });

  it("returns a clear error for unknown slash commands", async () => {
    const harness = createHandlerHarness();

    const result = await harness.handler(
      { command: "unknown", prompt: "" } as vscode.ChatRequest,
      { history: [] } as vscode.ChatContext,
      harness.streamCapture,
      createToken(),
    );

    expect(result).toMatchObject({
      errorDetails: {
        message: "Unknown Tomcat slash command: /unknown",
      },
      metadata: {
        sessionId: "s1",
      },
    });
    expect(harness.messenger.setPlanModeCalls).toHaveLength(0);
    expect(harness.messenger.listModelCalls).toHaveLength(0);
  });
});
