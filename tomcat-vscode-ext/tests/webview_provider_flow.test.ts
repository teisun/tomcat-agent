import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import type { InitializeResult } from "../src/serveClient/initialize";
import { SessionOwnershipTracker } from "../src/ui/webview/ownership";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerFile(filePath: string, text: string): void;
      reset(): void;
      setOpenDialogHandler(
        handler: ((options: unknown) => vscode.Uri[] | Promise<vscode.Uri[] | undefined> | undefined) | undefined,
      ): void;
    };
  }
).__testing;

type MutableSessionState = {
  busy: boolean;
  model: string;
  modelThinking: Record<string, string | null>;
  planId: string | null;
  planPath: string | null;
  planState: string;
  thinkingLevel: string | null;
};

class FakeMessenger {
  readonly requestCalls: Array<Record<string, unknown>> = [];
  readonly setPlanModeCalls: Array<Record<string, unknown>> = [];
  readonly setThinkingLevelCalls: Array<{
    level: string;
    model: string;
    sessionId: string | null | undefined;
  }> = [];
  private readonly listeners = new Set<(event: Record<string, unknown>) => void>();

  constructor(private readonly sessionState: MutableSessionState) {}

  emit(event: Record<string, unknown>): void {
    for (const listener of this.listeners) {
      listener(event);
    }
  }

  onEvent(listener: (event: Record<string, unknown>) => void) {
    this.listeners.add(listener);
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  }

  async request(command: Record<string, unknown>) {
    this.requestCalls.push(command);
    return {
      payload: { accepted: true },
      sessionId: String(command.sessionId ?? "session-1"),
      success: true,
      type: "response",
    };
  }

  async sendListModels() {
    return {
      payload: { models: [{ id: "gpt-5.4" }, { id: "claude-4.6-sonnet" }] },
      success: true,
      type: "response",
    };
  }

  async sendSetModel(_sessionId: string | null | undefined, model: string) {
    this.sessionState.model = model;
    this.sessionState.thinkingLevel = this.sessionState.modelThinking[model] ?? null;
    return {
      payload: { model },
      success: true,
      type: "response",
    };
  }

  async sendSetThinkingLevel(
    sessionId: string | null | undefined,
    model: string,
    level: "high" | "low" | "medium" | "xhigh",
  ) {
    this.setThinkingLevelCalls.push({ level, model, sessionId });
    this.sessionState.modelThinking[model] = level;
    if (this.sessionState.model === model) {
      this.sessionState.thinkingLevel = level;
    }
    return {
      payload: { level, model },
      success: true,
      type: "response",
    };
  }

  async sendSetPlanMode(command: Record<string, unknown>) {
    this.setPlanModeCalls.push(command);
    const sessionId = String(command.sessionId ?? "session-1");
    if (command.action === "enter") {
      this.sessionState.planId = "plan-1";
      this.sessionState.planPath = "/workspace/plans/plan-1.plan.md";
      this.sessionState.planState = "planning";
      this.emit({
        path: this.sessionState.planPath,
        planId: this.sessionState.planId,
        sessionId,
        state: this.sessionState.planState,
        type: "plan.create",
      });
    } else if (command.action === "build") {
      this.sessionState.planId = this.sessionState.planId ?? "plan-1";
      this.sessionState.planPath = this.sessionState.planPath ?? "/workspace/plans/plan-1.plan.md";
      this.sessionState.planState = "executing";
      this.emit({
        path: this.sessionState.planPath,
        planId: this.sessionState.planId,
        sessionId,
        state: this.sessionState.planState,
        type: "plan.build",
      });
    } else {
      this.sessionState.planState = "chat";
      this.emit({
        path: this.sessionState.planPath,
        planId: this.sessionState.planId,
        sessionId,
        state: "completed",
        type: "plan.complete",
      });
      this.sessionState.planId = null;
    }
    return {
      payload: {
        planId: this.sessionState.planId,
        planState: this.sessionState.planState,
      },
      success: true,
      type: "response",
    };
  }
}

function initializeResult(): InitializeResult {
  return {
    capabilities: [
      "ask_question",
      "prompt",
      "list_models",
      "set_plan_mode",
      "set_thinking_level",
    ],
    protocolVersion: 1,
    sessionId: "session-1",
  };
}

function buildProvider() {
  __testing.reset();

  const sessionState: MutableSessionState = {
    busy: false,
    model: "gpt-5.4",
    modelThinking: {
      "claude-4.6-sonnet": "low",
      "gpt-5.4": "high",
    },
    planId: null,
    planPath: null,
    planState: "chat",
    thinkingLevel: "high",
  };
  const messenger = new FakeMessenger(sessionState);
  const sessionRouter = {
    buildResultMetadata(sessionId: string) {
      return { sessionId };
    },
    async closeSession() {
      return true;
    },
    async getMessages(sessionId?: string) {
      return {
        messages: [
          {
            id: "hist-user-1",
            message: {
              content: "restored prompt",
              role: "user",
            },
            type: "message",
          },
          {
            id: "hist-assistant-1",
            message: {
              content: "restored answer",
              role: "assistant",
            },
            type: "message",
          },
        ],
        sessionId: sessionId ?? "session-1",
        upToSeq: null,
      };
    },
    async getState(sessionId?: string) {
      return {
        busy: sessionState.busy,
        model: sessionState.model,
        planId: sessionState.planId,
        planState: sessionState.planState,
        sessionId: sessionId ?? "session-1",
        thinkingLevel: sessionState.thinkingLevel,
      };
    },
    async listSessions() {
      return {
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          {
            busy: false,
            isCurrent: true,
            sessionId: "session-1",
            updatedAt: Date.now(),
          },
        ],
      };
    },
    async newSession() {
      return "session-1";
    },
    async resolveSessionId() {
      return "session-1";
    },
    async switchSession(sessionId: string) {
      return sessionId;
    },
  };

  const provider = new TomcatWebviewViewProvider({
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
      showFile: async () => undefined,
    } as never,
    initialize: async () => initializeResult(),
    messenger: messenger as never,
    ownership: new SessionOwnershipTracker(),
    sessionRouter: sessionRouter as never,
  });

  return { messenger, provider, sessionState };
}

describe("webview provider integration", () => {
  it("hydrates history during bootstrap and carries attachments through prompt requests", async () => {
    const { messenger, provider } = buildProvider();
    __testing.registerFile("/workspace/diagram.png", "png-bytes");
    __testing.setOpenDialogHandler(() => [vscode.Uri.file("/workspace/diagram.png")]);

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "restored prompt", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "restored answer", type: "message" }),
      ]),
    );

    await provider.dispatchTestIntent({
      messageId: "pick-1",
      type: "pickAttachment",
    });
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments[0]).toMatchObject({
      kind: "image",
      label: "diagram.png",
      mimeType: "image/png",
    });

    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "send with attachment",
      },
      messageId: "prompt-1",
      type: "prompt",
    });

    expect(messenger.requestCalls).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          params: {
            attachments: [
              expect.objectContaining({
                dataBase64: Buffer.from("png-bytes", "utf8").toString("base64"),
                kind: "image",
                mimeType: "image/png",
              }),
            ],
          },
          sessionId: "session-1",
          text: "send with attachment",
          type: "prompt",
        }),
      ]),
    );
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments).toHaveLength(0);

    provider.dispose();
  });

  it("tracks enter, build, and exit plan state through provider intents", async () => {
    const { messenger, provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        action: "enter",
        sessionId: "session-1",
      },
      messageId: "plan-enter-1",
      type: "setPlanMode",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        planId: "plan-1",
        state: "planning",
      },
      planId: "plan-1",
      planState: "planning",
    });

    await provider.dispatchTestIntent({
      data: {
        action: "build",
        planId: "plan-1",
        sessionId: "session-1",
      },
      messageId: "plan-build-1",
      type: "setPlanMode",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        planId: "plan-1",
        state: "executing",
      },
      planState: "executing",
    });

    await provider.dispatchTestIntent({
      data: {
        action: "exit",
        planId: "plan-1",
        sessionId: "session-1",
      },
      messageId: "plan-exit-1",
      type: "setPlanMode",
    });
    expect(provider.currentState().sessionViews["session-1"]?.planState).toBe("chat");
    expect(messenger.setPlanModeCalls.map((call) => call.action)).toEqual([
      "enter",
      "build",
      "exit",
    ]);

    provider.dispose();
  });

  it("resolves batched askQuestion answers through the provider roundtrip", async () => {
    const { provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    const responsePromise = provider.askUser(
      {
        questions: [
          {
            id: "q1",
            options: [
              { id: "day", label: "Day", recommended: true },
              { id: "night", label: "Night" },
            ],
            prompt: "When do you prefer to code?",
          },
          {
            id: "q2",
            options: [
              { id: "ts", label: "TypeScript", recommended: true },
              { id: "rs", label: "Rust" },
            ],
            prompt: "Which language do you want to use?",
          },
        ],
        requestId: "ask-1",
        responseEvent: "response-ask-1",
      },
      "session-1",
    );

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          request: expect.objectContaining({ requestId: "ask-1" }),
          resolved: false,
          type: "approval",
        }),
      ]),
    );

    await provider.dispatchTestIntent({
      data: {
        requestId: "ask-1",
        result: {
          answers: [
            {
              optionIds: ["day"],
              pickedRecommended: true,
              questionId: "q1",
            },
            {
              customText: "Rust",
              optionIds: ["__custom__"],
              pickedRecommended: false,
              questionId: "q2",
            },
          ],
          cancelled: false,
        },
      },
      messageId: "answer-question-1",
      type: "answerQuestion",
    });

    await expect(responsePromise).resolves.toEqual({
      requestId: "ask-1",
      result: {
        answers: [
          {
            optionIds: ["day"],
            pickedRecommended: true,
            questionId: "q1",
          },
          {
            customText: "Rust",
            optionIds: ["__custom__"],
            pickedRecommended: false,
            questionId: "q2",
          },
        ],
        cancelled: false,
      },
    });
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          request: expect.objectContaining({ requestId: "ask-1" }),
          resolved: true,
          type: "approval",
        }),
      ]),
    );

    provider.dispose();
  });

  it("resolves cancelled askQuestion answers through the provider roundtrip", async () => {
    const { provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    const responsePromise = provider.askUser(
      {
        questions: [
          {
            id: "q1",
            options: [{ id: "yes", label: "Yes", recommended: true }],
            prompt: "Proceed?",
          },
        ],
        requestId: "ask-2",
        responseEvent: "response-ask-2",
      },
      "session-1",
    );

    await provider.dispatchTestIntent({
      data: {
        requestId: "ask-2",
        result: {
          answers: [],
          cancelled: true,
        },
      },
      messageId: "answer-question-2",
      type: "answerQuestion",
    });

    await expect(responsePromise).resolves.toEqual({
      requestId: "ask-2",
      result: {
        answers: [],
        cancelled: true,
      },
    });

    provider.dispose();
  });

  it("surfaces prompt bridge timeouts as user-visible errors", async () => {
    const { messenger, provider } = buildProvider();
    vi.spyOn(messenger, "request").mockRejectedValue(
      new Error("Timed out waiting for response prompt-timeout"),
    );

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "will timeout",
      },
      messageId: "prompt-timeout-1",
      type: "prompt",
    });

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "error",
          text: expect.stringContaining("Tomcat bridge is not responding"),
          type: "message",
        }),
      ]),
    );

    provider.dispose();
  });

  it("surfaces setModel bridge exits as user-visible errors", async () => {
    const { messenger, provider, sessionState } = buildProvider();
    vi.spyOn(messenger, "sendSetModel").mockRejectedValue(
      new Error("tomcat serve exited (code=1, signal=null)"),
    );

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        modelId: "claude-4.6-sonnet",
        sessionId: "session-1",
      },
      messageId: "set-model-error-1",
      type: "setModel",
    });

    expect(sessionState.model).toBe("gpt-5.4");
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "error",
          text: expect.stringContaining("Tomcat serve exited"),
          type: "message",
        }),
      ]),
    );

    provider.dispose();
  });

  it("roundtrips setThinkingLevel through provider refresh", async () => {
    const { messenger, provider, sessionState } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        level: "xhigh",
        modelId: "gpt-5.4",
        sessionId: "session-1",
      },
      messageId: "set-thinking-level-1",
      type: "setThinkingLevel",
    });

    expect(messenger.setThinkingLevelCalls).toEqual([
      {
        level: "xhigh",
        model: "gpt-5.4",
        sessionId: "session-1",
      },
    ]);
    expect(sessionState.thinkingLevel).toBe("xhigh");
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      model: "gpt-5.4",
      thinkingLevel: "xhigh",
    });

    provider.dispose();
  });

  it("updates thinkingLevel from getState when switching models", async () => {
    const { provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      model: "gpt-5.4",
      thinkingLevel: "high",
    });

    await provider.dispatchTestIntent({
      data: {
        modelId: "claude-4.6-sonnet",
        sessionId: "session-1",
      },
      messageId: "set-model-1",
      type: "setModel",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      model: "claude-4.6-sonnet",
      thinkingLevel: "low",
    });

    await provider.dispatchTestIntent({
      data: {
        modelId: "gpt-5.4",
        sessionId: "session-1",
      },
      messageId: "set-model-2",
      type: "setModel",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      model: "gpt-5.4",
      thinkingLevel: "high",
    });

    provider.dispose();
  });
});
