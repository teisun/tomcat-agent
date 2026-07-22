import { describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import type { InitializeResult } from "../src/serveClient/initialize";
import type {
  RestoreCheckpointPayload,
  SessionCheckpointListPayload,
  SessionHistoryPayload,
} from "../src/serveClient/sessionRouter";
import { TomcatWebviewViewProvider } from "../src/ui/webview/provider";
import type { IdeHost } from "../src/ui/webview/types";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerDirectory(dirPath: string): void;
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
  contextRatio: number | null;
  interrupted: boolean;
  model: string;
  modelThinking: Record<string, string | null>;
  planId: string | null;
  planPath: string | null;
  planState: string;
  thinkingLevel: string | null;
};

type BuildProviderOptions = {
  getMessagesImpl?: (
    sessionId?: string,
    params?: { cursor?: string | null; limit?: number },
  ) => Promise<SessionHistoryPayload>;
  getStateImpl?: (sessionId?: string) => Promise<Record<string, unknown>>;
  historyMessages?: unknown[];
  historyResponses?: Record<string, SessionHistoryPayload>;
  ideOverrides?: Partial<IdeHost>;
  listCheckpointsImpl?: (sessionId?: string) => Promise<SessionCheckpointListPayload>;
  listModelsPayload?: Record<string, unknown>;
  listSessionsImpl?: () => Promise<Record<string, unknown>>;
  openModelSettings?: (route?: "models") => void;
  requestImpl?: (command: Record<string, unknown>) => Promise<Record<string, unknown>>;
  restoreCheckpointImpl?: (
    sessionId: string,
    checkpointId: string,
    revertFiles: boolean,
  ) => Promise<RestoreCheckpointPayload>;
  sessionState?: Partial<MutableSessionState>;
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
  listModelsPayload: Record<string, unknown> = {
    models: [{ id: "gpt-5.4" }, { id: "claude-4.6-sonnet" }],
  };

  constructor(
    private readonly sessionState: MutableSessionState,
    private readonly requestImpl?: (command: Record<string, unknown>) => Promise<Record<string, unknown>>,
  ) {}

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
    if (this.requestImpl) {
      return this.requestImpl(command);
    }
    return {
      payload: { accepted: true },
      sessionId: String(command.sessionId ?? "session-1"),
      success: true,
      type: "response",
    };
  }

  async sendListModels() {
    return {
      payload: this.listModelsPayload,
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
      "list_provider_keys",
      "prompt",
      "remove_model",
      "list_models",
      "set_provider_key",
      "set_plan_mode",
      "set_thinking_level",
      "upsert_model",
    ],
    protocolVersion: 1,
    sessionId: "session-1",
  };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

function buildProvider(options: BuildProviderOptions = {}) {
  __testing.reset();

  const sessionState: MutableSessionState = {
    busy: false,
    contextRatio: null,
    interrupted: false,
    model: "gpt-5.4",
    modelThinking: {
      "claude-4.6-sonnet": "low",
      "gpt-5.4": "high",
    },
    planId: null,
    planPath: null,
    planState: "chat",
    thinkingLevel: "high",
    ...options.sessionState,
  };
  const messenger = new FakeMessenger(sessionState, options.requestImpl);
  const historyCalls: Array<{
    params?: { cursor?: string | null; limit?: number };
    sessionId?: string;
  }> = [];
  const sessionRouter = {
    buildResultMetadata(sessionId: string) {
      return { sessionId };
    },
    async closeSession() {
      return true;
    },
    async getMessages(sessionId?: string, params?: { cursor?: string | null; limit?: number }) {
      historyCalls.push({ params, sessionId });
      if (options.getMessagesImpl) {
        return options.getMessagesImpl(sessionId, params);
      }
      const response =
        (params?.cursor ? options.historyResponses?.[params.cursor] : options.historyResponses?.__latest__) ??
        ({
          messages: options.historyMessages ?? [
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
        } satisfies SessionHistoryPayload);
      return {
        ...response,
        sessionId: response.sessionId ?? sessionId ?? "session-1",
      };
    },
    async getState(sessionId?: string) {
      if (options.getStateImpl) {
        return options.getStateImpl(sessionId);
      }
      return {
        busy: sessionState.busy,
        contextRatio: sessionState.contextRatio,
        interrupted: sessionState.interrupted,
        model: sessionState.model,
        planId: sessionState.planId,
        planPath: sessionState.planPath,
        planState: sessionState.planState,
        sessionId: sessionId ?? "session-1",
        thinkingLevel: sessionState.thinkingLevel,
      };
    },
    async listSessions() {
      if (options.listSessionsImpl) {
        return options.listSessionsImpl();
      }
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
    async listCheckpoints(sessionId?: string) {
      if (options.listCheckpointsImpl) {
        return options.listCheckpointsImpl(sessionId);
      }
      return {
        checkpoints: [],
        sessionId: sessionId ?? "session-1",
      } satisfies SessionCheckpointListPayload;
    },
    async resolveSessionId() {
      return "session-1";
    },
    async restoreCheckpoint(sessionId: string, checkpointId: string, revertFiles: boolean) {
      if (options.restoreCheckpointImpl) {
        return options.restoreCheckpointImpl(sessionId, checkpointId, revertFiles);
      }
      return {
        changedPaths: [],
        checkpointId,
        createdAt: "2026-07-12T12:00:00Z",
        dryRun: false,
        kind: "turn_end",
        restoredPaths: [],
        revertFiles,
        sessionId,
        transcriptTruncated: true,
        warnings: [],
      } satisfies RestoreCheckpointPayload;
    },
    async switchSession(sessionId: string) {
      return sessionId;
    },
  };
  const openModelSettings = options.openModelSettings ?? vi.fn();

  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/extension"),
    getDefaultCwd: () => "/workspace",
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
      ...options.ideOverrides,
    } as never,
    initialize: async () => initializeResult(),
    messenger: messenger as never,
    openModelSettings,
    sessionRouter: sessionRouter as never,
  });

  messenger.listModelsPayload = options.listModelsPayload ?? messenger.listModelsPayload;
  return { historyCalls, messenger, provider, sessionState };
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
      type: "pickContext",
    });
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments[0]).toMatchObject({
      attachment: {
        filename: "diagram.png",
      },
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

    const promptRequest = messenger.requestCalls.find((call) => call.type === "prompt");
    expect(promptRequest).toEqual(
      expect.objectContaining({
        params: {
          attachments: [
            expect.objectContaining({
              dataBase64: Buffer.from("png-bytes", "utf8").toString("base64"),
              filename: "diagram.png",
              kind: "image",
              mimeType: "image/png",
            }),
          ],
          userMessageId: expect.any(String),
        },
        sessionId: "session-1",
        text: "send with attachment",
        type: "prompt",
      }),
    );
    const userMessageId = (promptRequest?.params as { userMessageId?: string } | undefined)?.userMessageId;
    const sentUserMessage = provider.currentState().sessionViews["session-1"]?.timeline.find(
      (item) => item.type === "message" && item.id === userMessageId,
    );
    expect(sentUserMessage).toMatchObject({
      id: userMessageId,
      kind: "user",
      submitKind: "prompt",
      text: "send with attachment",
      type: "message",
    });
    expect(sentUserMessage).not.toHaveProperty("deliveryError");
    expect(sentUserMessage).not.toHaveProperty("deliveryState");
    expect(sentUserMessage).not.toHaveProperty("retryable");
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments).toHaveLength(0);

    provider.dispose();
  });

  it("routes mixed picker selections into attachments and references", async () => {
    const { provider } = buildProvider();
    __testing.registerFile("/workspace/diagram.png", "png-bytes");
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");
    __testing.registerDirectory("/workspace/src/folder");
    __testing.registerFile("/outside/log.txt", "outside-log");
    __testing.setOpenDialogHandler(() => [
      vscode.Uri.file("/workspace/diagram.png"),
      vscode.Uri.file("/workspace/src/app.ts"),
      vscode.Uri.file("/workspace/src/folder"),
      vscode.Uri.file("/outside/log.txt"),
    ]);

    await provider.dispatchTestIntent({
      messageId: "ready-pick-context",
      type: "ready",
    });

    const postInsertReference = vi
      .spyOn(provider, "postInsertReference")
      .mockResolvedValue(undefined);

    await provider.dispatchTestIntent({
      messageId: "pick-context-1",
      type: "pickContext",
    });

    expect(postInsertReference).toHaveBeenCalledTimes(3);
    expect(postInsertReference).toHaveBeenNthCalledWith(
      1,
      "session-1",
      {
        kind: "file",
        label: "app.ts",
        path: "src/app.ts",
        type: "reference",
      },
    );
    expect(postInsertReference).toHaveBeenNthCalledWith(
      2,
      "session-1",
      {
        kind: "file",
        label: "folder/",
        path: "src/folder/",
        type: "reference",
      },
    );
    expect(postInsertReference).toHaveBeenNthCalledWith(
      3,
      "session-1",
      {
        kind: "file",
        label: "log.txt",
        path: "/outside/log.txt",
        type: "reference",
      },
    );
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments).toEqual([
      expect.objectContaining({
        attachment: expect.objectContaining({
          dataBase64: Buffer.from("png-bytes", "utf8").toString("base64"),
          filename: "diagram.png",
          kind: "image",
          mimeType: "image/png",
        }),
        kind: "image",
        label: "diagram.png",
        mimeType: "image/png",
        path: "/workspace/diagram.png",
      }),
    ]);

    const attachmentId = provider.currentState().sessionViews["session-1"]?.pendingAttachments[0]?.id;
    expect(attachmentId).toBeTruthy();

    await provider.dispatchTestIntent({
      data: {
        attachmentId: attachmentId ?? "missing-attachment-id",
        sessionId: "session-1",
      },
      messageId: "remove-picked-attachment",
      type: "removeAttachment",
    });

    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments).toEqual([]);
    provider.dispose();
  });

  it("passes ordered message segments through prompt requests and optimistic history", async () => {
    const { messenger, provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-segments",
      type: "ready",
    });

    const segments = [
      { text: "Inspect ", type: "text" as const },
      {
        kind: "selection" as const,
        label: "app.ts:3-5",
        lineEnd: 5,
        lineStart: 3,
        path: "app.ts",
        text: "const answer = 42;",
        type: "reference" as const,
      },
      { text: " carefully", type: "text" as const },
    ];

    await provider.dispatchTestIntent({
      data: {
        segments,
        sessionId: "session-1",
        text: "Inspect app.ts:3-5 carefully",
      },
      messageId: "prompt-segments",
      type: "prompt",
    });

    const promptRequest = messenger.requestCalls.find((call) => call.type === "prompt");
    expect(promptRequest).toEqual(
      expect.objectContaining({
        params: expect.objectContaining({
          segments,
        }),
        sessionId: "session-1",
        text: "Inspect app.ts:3-5 carefully",
      }),
    );

    const userMessageId = (promptRequest?.params as { userMessageId?: string } | undefined)?.userMessageId;
    expect(
      provider.currentState().sessionViews["session-1"]?.timeline.find(
        (item) => item.type === "message" && item.id === userMessageId,
      ),
    ).toMatchObject({
      id: userMessageId,
      kind: "user",
      segments,
      submitKind: "prompt",
      text: "Inspect app.ts:3-5 carefully",
      type: "message",
    });

    provider.dispose();
  });

  it("shapes @ search references like dropped file references and preserves them in prompt segments", async () => {
    const { messenger, provider } = buildProvider();
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");

    const postEventSpy = vi
      .spyOn(provider as unknown as { postEvent(content: Record<string, unknown>): Promise<void> }, "postEvent")
      .mockResolvedValue(undefined);
    const postInsertReference = vi
      .spyOn(provider, "postInsertReference")
      .mockResolvedValue(undefined);

    await provider.dispatchTestIntent({
      data: {
        query: "app",
        requestId: "req-at-1",
        sessionId: "session-1",
      },
      messageId: "search-at-1",
      type: "searchContext",
    });

    const searchEvent = postEventSpy.mock.calls
      .map(([content]) => content)
      .find((content) => content.type === "contextSearchResult");
    const searchReference = (searchEvent as {
      matches: Array<{ reference: Record<string, unknown> }>;
    }).matches[0]?.reference;

    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        uris: [vscode.Uri.file("/workspace/src/app.ts").toString()],
      },
      messageId: "drop-shape-1",
      type: "resolveDrop",
    });

    const droppedReference = postInsertReference.mock.calls[0]?.[1];
    expect(searchReference).toEqual(droppedReference);

    const segments = [
      { text: "Inspect ", type: "text" as const },
      searchReference as {
        kind: "file";
        label: string;
        path: string;
        type: "reference";
      },
      { text: " now", type: "text" as const },
    ];

    await provider.dispatchTestIntent({
      data: {
        segments,
        sessionId: "session-1",
        text: "Inspect app.ts now",
      },
      messageId: "prompt-at-shape",
      type: "prompt",
    });

    expect(messenger.requestCalls.find((call) => call.type === "prompt")).toEqual(
      expect.objectContaining({
        params: expect.objectContaining({
          segments,
        }),
      }),
    );

    postEventSpy.mockRestore();
    postInsertReference.mockRestore();
    provider.dispose();
  });

  it("routes dropped uris into matching reference and attachment channels", async () => {
    const { provider } = buildProvider();
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");
    __testing.registerFile("/workspace/assets/mockup.png", "png-bytes");
    __testing.registerDirectory("/workspace/src/folder");

    await provider.dispatchTestIntent({
      messageId: "ready-drop",
      type: "ready",
    });

    const postInsertReference = vi
      .spyOn(provider, "postInsertReference")
      .mockResolvedValue(undefined);

    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        uris: [
          vscode.Uri.file("/workspace/src/app.ts").toString(),
          vscode.Uri.file("/workspace/assets/mockup.png").toString(),
          vscode.Uri.file("/workspace/src/folder").toString(),
        ],
      },
      messageId: "drop-1",
      type: "resolveDrop",
    });

    expect(postInsertReference).toHaveBeenCalledTimes(2);
    expect(postInsertReference).toHaveBeenNthCalledWith(
      1,
      "session-1",
      {
        kind: "file",
        label: "app.ts",
        path: "src/app.ts",
        type: "reference",
      },
    );
    expect(postInsertReference).toHaveBeenNthCalledWith(
      2,
      "session-1",
      {
        kind: "file",
        label: "folder/",
        path: "src/folder/",
        type: "reference",
      },
    );
    expect(provider.currentState().sessionViews["session-1"]?.pendingAttachments).toEqual([
      expect.objectContaining({
        attachment: expect.objectContaining({
          dataBase64: Buffer.from("png-bytes", "utf8").toString("base64"),
          filename: "mockup.png",
          kind: "image",
          mimeType: "image/png",
        }),
        kind: "image",
        label: "mockup.png",
        mimeType: "image/png",
        path: "/workspace/assets/mockup.png",
      }),
    ]);

    provider.dispose();
  });

  it("opens model settings when the composer footer intent fires", async () => {
    const openModelSettings = vi.fn();
    const { provider } = buildProvider({
      listModelsPayload: { models: [] },
      openModelSettings,
    });

    await provider.dispatchTestIntent({
      messageId: "ready-open-model-settings",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: { route: "models" },
      messageId: "open-model-settings",
      type: "openModelSettings",
    });

    expect(openModelSettings).toHaveBeenCalledWith("models");
    provider.dispose();
  });

  it("refreshes the available model catalog after external model changes", async () => {
    const { messenger, provider } = buildProvider({
      listModelsPayload: {
        models: [{ id: "gpt-5.4", keyPresent: true }],
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-refresh-model-catalog",
      type: "ready",
    });
    expect(provider.currentState().availableModels).toEqual(["gpt-5.4"]);

    messenger.listModelsPayload = {
      models: [
        { id: "gpt-5.4", keyPresent: true },
        { id: "claude-opus-gateway", keyPresent: true },
        { id: "missing-key", keyPresent: false },
      ],
    };

    await provider.refreshModelCatalog();

    expect(provider.currentState().availableModels).toEqual([
      "gpt-5.4",
      "claude-opus-gateway",
    ]);
    provider.dispose();
  });

  it("never surfaces plaintext provider keys in webview state snapshots", async () => {
    const { provider } = buildProvider({
      listModelsPayload: {
        models: [
          {
            apiKey: "relay-secret",
            id: "claude-opus-gateway",
            keyPresent: true,
          },
        ],
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-no-plaintext-state",
      type: "ready",
    });

    const snapshot = JSON.stringify(provider.currentState());
    expect(snapshot).not.toContain("relay-secret");
    expect(provider.currentState().availableModels).toEqual(["claude-opus-gateway"]);
    provider.dispose();
  });

  it("refreshes the session list after a prompt so generated titles appear", async () => {
    let listSessionsCalls = 0;
    const { provider } = buildProvider({
      listSessionsImpl: async () => {
        listSessionsCalls += 1;
        return {
          activeSessionId: "session-1",
          scope: "disk",
          sessions: [
            {
              busy: false,
              isCurrent: true,
              sessionId: "session-1",
              title: listSessionsCalls >= 2 ? "Transcript cleanup plan" : undefined,
              updatedAt: Date.now(),
            },
          ],
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-title-refresh",
      type: "ready",
    });
    const callsAfterReady = listSessionsCalls;

    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "Generate a better title",
      },
      messageId: "prompt-title-refresh",
      type: "prompt",
    });

    expect(listSessionsCalls).toBeGreaterThan(callsAfterReady);
    expect(provider.currentState().sessions[0]?.title).toBe("Transcript cleanup plan");
    provider.dispose();
  });

  it("keeps the user-selected session active when listSessions reports another running session", async () => {
    let listSessionsCalls = 0;
    const { messenger, provider } = buildProvider({
      listSessionsImpl: async () => {
        listSessionsCalls += 1;
        return {
          activeSessionId: listSessionsCalls >= 2 ? "session-2" : "session-1",
          scope: "disk",
          sessions: [
            {
              busy: false,
              isCurrent: listSessionsCalls < 2,
              sessionId: "session-1",
              title: "Session A",
              updatedAt: 1,
            },
            {
              busy: true,
              isCurrent: listSessionsCalls >= 2,
              sessionId: "session-2",
              title: "Session B",
              updatedAt: 2,
            },
          ],
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-keep-active-session",
      type: "ready",
    });

    expect(provider.currentState().activeSessionId).toBe("session-1");

    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "stay on A",
      },
      messageId: "prompt-keep-active-session",
      type: "prompt",
    });

    expect(messenger.requestCalls).toContainEqual(
      expect.objectContaining({
        sessionId: "session-1",
        text: "stay on A",
        type: "prompt",
      }),
    );
    expect(provider.currentState().activeSessionId).toBe("session-1");
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "stay on A", type: "message" }),
      ]),
    );

    provider.dispose();
  });

  it("keeps rejected user bubbles inline and retries them with the same stable id", async () => {
    let requestCount = 0;
    const { messenger, provider } = buildProvider({
      requestImpl: async (command) => {
        requestCount += 1;
        return requestCount === 1
          ? {
              error: "busy",
              sessionId: String(command.sessionId ?? "session-1"),
              success: false,
              type: "response",
            }
          : {
              payload: { accepted: true },
              sessionId: String(command.sessionId ?? "session-1"),
              success: true,
              type: "response",
            };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-retry-user-message",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "retry me",
      },
      messageId: "prompt-retry-user-message",
      type: "prompt",
    });

    const failedSession = provider.currentState().sessionViews["session-1"];
    const failedUserMessage = failedSession?.timeline.find(
      (item) => item.type === "message" && item.kind === "user" && item.text === "retry me",
    );
    expect(failedUserMessage).toMatchObject({
      deliveryError: "busy",
      deliveryState: "failed",
      retryable: true,
      submitKind: "prompt",
      text: "retry me",
      type: "message",
    });
    expect(
      failedSession?.timeline.some((item) => item.type === "message" && item.kind === "error"),
    ).toBe(false);

    const firstPromptRequest = messenger.requestCalls.find((call) => call.type === "prompt");
    const firstUserMessageId = (
      firstPromptRequest?.params as { userMessageId?: string } | undefined
    )?.userMessageId;
    expect(failedUserMessage?.id).toBe(firstUserMessageId);

    await provider.dispatchTestIntent({
      data: {
        messageId: String(firstUserMessageId),
        sessionId: "session-1",
      },
      messageId: "retry-user-message",
      type: "retryUserMessage",
    });

    const retriedRequests = messenger.requestCalls.filter((call) => call.type === "prompt");
    expect(retriedRequests).toHaveLength(2);
    expect(
      (
        retriedRequests[1]?.params as { userMessageId?: string } | undefined
      )?.userMessageId,
    ).toBe(firstUserMessageId);
    const retriedUserMessages =
      provider.currentState().sessionViews["session-1"]?.timeline.filter(
        (item) => item.type === "message" && item.kind === "user" && item.text === "retry me",
      ) ?? [];
    expect(retriedUserMessages).toHaveLength(1);
    expect(retriedUserMessages[0]).toMatchObject({
      id: firstUserMessageId,
      kind: "user",
      submitKind: "prompt",
      text: "retry me",
      type: "message",
    });
    expect(retriedUserMessages[0]).not.toHaveProperty("deliveryError");
    expect(retriedUserMessages[0]).not.toHaveProperty("deliveryState");
    expect(retriedUserMessages[0]).not.toHaveProperty("retryable");
  });

  it("clears sending immediately for queued steering messages", async () => {
    const { messenger, provider } = buildProvider({
      requestImpl: async (command) => ({
        payload: { queued: true },
        sessionId: String(command.sessionId ?? "session-1"),
        success: true,
        type: "response",
      }),
      sessionState: {
        busy: true,
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-queued-steer",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "keep going",
      },
      messageId: "steer-queued-message",
      type: "steer",
    });

    const steerRequest = messenger.requestCalls.find((call) => call.type === "steer");
    const userMessageId = (steerRequest?.params as { userMessageId?: string } | undefined)?.userMessageId;
    const steerMessage = provider.currentState().sessionViews["session-1"]?.timeline.find(
      (item) => item.type === "message" && item.id === userMessageId,
    );
    expect(steerMessage).toMatchObject({
      id: userMessageId,
      kind: "user",
      submitKind: "steer",
      text: "keep going",
      type: "message",
    });
    expect(steerMessage).not.toHaveProperty("deliveryError");
    expect(steerMessage).not.toHaveProperty("deliveryState");
    expect(steerMessage).not.toHaveProperty("retryable");
  });

  it("prepends older history pages with cursor pagination", async () => {
    const { historyCalls, provider } = buildProvider({
      historyResponses: {
        __latest__: {
          hasMore: true,
          messages: [
            {
              id: "hist-user-2",
              message: { content: "second prompt", role: "user" },
              type: "message",
            },
            {
              id: "hist-assistant-2",
              message: { content: "second answer", role: "assistant" },
              type: "message",
            },
          ],
          nextCursor: "cursor-1",
          sessionId: "session-1",
        },
        "cursor-1": {
          hasMore: false,
          messages: [
            {
              id: "hist-user-1",
              message: { content: "first prompt", role: "user" },
              type: "message",
            },
            {
              id: "hist-assistant-1",
              message: { content: "first answer", role: "assistant" },
              type: "message",
            },
          ],
          nextCursor: null,
          sessionId: "session-1",
        },
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-history-pages",
      type: "ready",
    });

    expect(historyCalls[0]).toMatchObject({
      params: { limit: 80 },
      sessionId: "session-1",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      hasMoreHistory: true,
      historyLoading: false,
    });

    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-history-1",
      type: "loadOlderHistory",
    });

    expect(historyCalls[1]).toMatchObject({
      params: { cursor: "cursor-1", limit: 80 },
      sessionId: "session-1",
    });
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      hasMoreHistory: false,
      historyLoading: false,
    });
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "first prompt", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "first answer", type: "message" }),
        expect.objectContaining({ kind: "user", text: "second prompt", type: "message" }),
        expect.objectContaining({ kind: "assistant", text: "second answer", type: "message" }),
      ]),
    );
  });

  it("falls back to single-page history when the server omits cursor metadata", async () => {
    const { historyCalls, provider } = buildProvider({
      historyResponses: {
        __latest__: {
          messages: [
            {
              id: "hist-user-1",
              message: { content: "single prompt", role: "user" },
              type: "message",
            },
          ],
          sessionId: "session-1",
        },
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-old-server",
      type: "ready",
    });

    expect(historyCalls).toHaveLength(1);
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      hasMoreHistory: false,
      historyLoading: false,
    });
  });

  it("restores current state before deferred history resolves during bootstrap", async () => {
    const historyResponse = deferred<SessionHistoryPayload>();
    const { provider } = buildProvider({
      getMessagesImpl: async () => historyResponse.promise,
      getStateImpl: async (sessionId) => ({
        busy: false,
        contextRatio: 0.42,
        model: "gpt-5.4",
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
    });

    const readyPromise = provider.dispatchTestIntent({
      messageId: "ready-state-first",
      type: "ready",
    });

    for (let i = 0; i < 20; i += 1) {
      if (provider.currentState().sessionViews["session-1"]) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }

    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      contextRatio: 0.42,
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        planId: "plan-1",
        state: "executing",
      },
      planId: "plan-1",
      planState: "executing",
    });
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual([]);
    expect(
      provider.currentState().sessionViews["session-1"]?.timeline.some(
        (item) => item.type === "message" && item.kind === "user",
      ),
    ).toBe(false);

    historyResponse.resolve({
      messages: [
        {
          id: "hist-user-1",
          message: { content: "restored prompt", role: "user" },
          type: "message",
        },
      ],
      sessionId: "session-1",
    });
    await readyPromise;

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "user", text: "restored prompt", type: "message" }),
      ]),
    );
  });

  it("guards against duplicate in-flight older-history requests", async () => {
    const olderResponse = deferred<SessionHistoryPayload>();
    const { historyCalls, provider } = buildProvider({
      getMessagesImpl: async (_sessionId, params) => {
        if (params?.cursor === "cursor-1") {
          return olderResponse.promise;
        }
        return {
          hasMore: true,
          messages: [
            {
              id: "hist-user-2",
              message: { content: "second prompt", role: "user" },
              type: "message",
            },
          ],
          nextCursor: "cursor-1",
          sessionId: "session-1",
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-inflight-guard",
      type: "ready",
    });

    const first = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-inflight-1",
      type: "loadOlderHistory",
    });
    const second = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-inflight-2",
      type: "loadOlderHistory",
    });

    for (let i = 0; i < 4; i += 1) {
      await Promise.resolve();
    }

    expect(
      historyCalls.filter((call) => call.params?.cursor === "cursor-1"),
    ).toHaveLength(1);
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      historyLoading: true,
    });

    olderResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-user-1",
          message: { content: "first prompt", role: "user" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });

    await Promise.all([first, second]);
    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      hasMoreHistory: false,
      historyLoading: false,
    });
  });

  it("clears stale older-history loading when a newer history refresh wins", async () => {
    const olderResponse = deferred<SessionHistoryPayload>();
    const { provider } = buildProvider({
      getMessagesImpl: async (sessionId, params) => {
        if (params?.cursor === "cursor-1") {
          return olderResponse.promise;
        }
        if (sessionId === "session-2") {
          return {
            hasMore: false,
            messages: [
              {
                id: "hist-session-b-1",
                message: { content: "session B", role: "assistant" },
                type: "message",
              },
            ],
            nextCursor: null,
            sessionId: "session-2",
          };
        }
        return {
          hasMore: true,
          messages: [
            {
              id: "hist-user-2",
              message: { content: "second prompt", role: "user" },
              type: "message",
            },
          ],
          nextCursor: "cursor-1",
          sessionId: "session-1",
        };
      },
      getStateImpl: async (sessionId) => ({
        busy: false,
        contextRatio: null,
        interrupted: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
      listSessionsImpl: async () => ({
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          { busy: false, isCurrent: true, sessionId: "session-1", updatedAt: 1 },
          { busy: false, isCurrent: false, sessionId: "session-2", updatedAt: 2 },
        ],
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-stale-older-history",
      type: "ready",
    });

    const loadingOlder = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-stale-history-1",
      type: "loadOlderHistory",
    });
    for (let i = 0; i < 4; i += 1) {
      await Promise.resolve();
    }

    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      historyLoading: true,
    });

    await provider.dispatchTestIntent({
      data: { sessionId: "session-2" },
      messageId: "switch-stale-older-to-b",
      type: "switchSession",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "switch-stale-older-back-to-a",
      type: "switchSession",
    });

    olderResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-user-1",
          message: { content: "older prompt", role: "user" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });
    await loadingOlder;

    expect(provider.currentState().sessionViews["session-1"]).toMatchObject({
      historyLoading: false,
    });
  });

  it("replays historical plan notices without overwriting current plan state", async () => {
    const { provider } = buildProvider({
      historyResponses: {
        __latest__: {
          hasMore: true,
          messages: [],
          nextCursor: "cursor-plan",
          sessionId: "session-1",
        },
        "cursor-plan": {
          hasMore: false,
          messages: [
            {
              event: "plan.review",
              id: "review-1",
              plan_id: "plan-1",
              summary: "looks good",
              type: "custom",
            },
          ],
          nextCursor: null,
          sessionId: "session-1",
        },
      },
      sessionState: {
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-plan-replay",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-plan-replay",
      type: "loadOlderHistory",
    });

    const session = provider.currentState().sessionViews["session-1"];
    expect(session).toMatchObject({
      planId: "plan-1",
      planState: "executing",
    });
    expect(session?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "notice",
          text: "Tomcat plan review: looks good",
          type: "message",
        }),
      ]),
    );
  });

  it("preserves live events that arrive while older history is still loading", async () => {
    const olderResponse = deferred<SessionHistoryPayload>();
    const { messenger, provider } = buildProvider({
      getMessagesImpl: async (_sessionId, params) => {
        if (params?.cursor === "cursor-live") {
          return olderResponse.promise;
        }
        return {
          hasMore: true,
          messages: [
            {
              id: "hist-user-2",
              message: { content: "second prompt", role: "user" },
              type: "message",
            },
          ],
          nextCursor: "cursor-live",
          sessionId: "session-1",
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-live-paginate",
      type: "ready",
    });

    const loadingOlder = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "older-live-paginate",
      type: "loadOlderHistory",
    });
    for (let i = 0; i < 4; i += 1) {
      await Promise.resolve();
    }

    messenger.emit({
      assistantMessageId: "live-assistant-1",
      assistantMessageEvent: { delta: "live answer", kind: "content_delta" },
      message: {},
      sessionId: "session-1",
      type: "message_update",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "assistant", text: "live answer", type: "message" }),
      ]),
    );

    olderResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-user-1",
          message: { content: "first prompt", role: "user" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });
    await loadingOlder;

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "assistant", text: "live answer", type: "message" }),
        expect.objectContaining({ kind: "user", text: "first prompt", type: "message" }),
        expect.objectContaining({ kind: "user", text: "second prompt", type: "message" }),
      ]),
    );
  });

  it("drops stale latest-history responses when a newer refresh wins after switching away and back", async () => {
    const firstSessionAResponse = deferred<SessionHistoryPayload>();
    const secondSessionAResponse = deferred<SessionHistoryPayload>();
    let sessionALatestCalls = 0;

    const { historyCalls, provider } = buildProvider({
      getMessagesImpl: async (sessionId, params) => {
        if (params?.cursor) {
          throw new Error("unexpected older-history request");
        }
        if (sessionId === "session-1") {
          sessionALatestCalls += 1;
          return sessionALatestCalls === 1
            ? firstSessionAResponse.promise
            : secondSessionAResponse.promise;
        }
        return {
          hasMore: false,
          messages: [
            {
              id: "hist-session-b-1",
              message: { content: "session B ready", role: "assistant" },
              type: "message",
            },
          ],
          nextCursor: null,
          sessionId: "session-2",
        };
      },
      getStateImpl: async (sessionId) => ({
        busy: false,
        contextRatio: null,
        interrupted: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
      listSessionsImpl: async () => ({
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          { busy: false, isCurrent: true, sessionId: "session-1", updatedAt: 1 },
          { busy: false, isCurrent: false, sessionId: "session-2", updatedAt: 2 },
        ],
      }),
    });

    const readyPromise = provider.dispatchTestIntent({
      messageId: "ready-stale-latest-history",
      type: "ready",
    });

    for (let i = 0; i < 20; i += 1) {
      if (provider.currentState().sessions.length >= 2) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }

    await provider.dispatchTestIntent({
      data: { sessionId: "session-2" },
      messageId: "switch-stale-latest-to-b",
      type: "switchSession",
    });

    const switchBackPromise = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "switch-stale-latest-back-to-a",
      type: "switchSession",
    });

    for (let i = 0; i < 20; i += 1) {
      if (historyCalls.filter((call) => call.sessionId === "session-1" && !call.params?.cursor).length >= 2) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }

    firstSessionAResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-session-a-stale",
          message: { content: "stale history should be dropped", role: "assistant" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });
    secondSessionAResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-session-a-fresh",
          message: { content: "fresh history wins", role: "assistant" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });

    await Promise.all([readyPromise, switchBackPromise]);

    const timeline = provider.currentState().sessionViews["session-1"]?.timeline ?? [];
    expect(
      timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "assistant" &&
          item.text === "stale history should be dropped",
      ),
    ).toBe(false);
    expect(
      timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "assistant" &&
          item.text === "fresh history wins",
      ),
    ).toBe(true);
  });

  it("drops stale older-history pages after a newer session refresh rebuilds the view", async () => {
    const olderResponse = deferred<SessionHistoryPayload>();
    let sessionALatestCalls = 0;

    const { provider } = buildProvider({
      getMessagesImpl: async (sessionId, params) => {
        if (sessionId === "session-2") {
          return {
            hasMore: false,
            messages: [],
            nextCursor: null,
            sessionId: "session-2",
          };
        }
        if (params?.cursor === "cursor-1") {
          return olderResponse.promise;
        }
        sessionALatestCalls += 1;
        return sessionALatestCalls === 1
          ? {
              hasMore: true,
              messages: [
                {
                  id: "hist-session-a-latest-1",
                  message: { content: "latest page before switch", role: "user" },
                  type: "message",
                },
              ],
              nextCursor: "cursor-1",
              sessionId: "session-1",
            }
          : {
              hasMore: false,
              messages: [
                {
                  id: "hist-session-a-latest-2",
                  message: { content: "rebuilt after switch back", role: "user" },
                  type: "message",
                },
              ],
              nextCursor: null,
              sessionId: "session-1",
            };
      },
      getStateImpl: async (sessionId) => ({
        busy: false,
        contextRatio: null,
        interrupted: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
      listSessionsImpl: async () => ({
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          { busy: false, isCurrent: true, sessionId: "session-1", updatedAt: 1 },
          { busy: false, isCurrent: false, sessionId: "session-2", updatedAt: 2 },
        ],
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-stale-older-history",
      type: "ready",
    });

    const loadOlderPromise = provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "load-stale-older-history",
      type: "loadOlderHistory",
    });

    for (let i = 0; i < 4; i += 1) {
      await Promise.resolve();
    }

    await provider.dispatchTestIntent({
      data: { sessionId: "session-2" },
      messageId: "switch-stale-older-to-b",
      type: "switchSession",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "switch-stale-older-back-to-a",
      type: "switchSession",
    });

    olderResponse.resolve({
      hasMore: false,
      messages: [
        {
          id: "hist-session-a-stale-older",
          message: { content: "stale older page", role: "user" },
          type: "message",
        },
      ],
      nextCursor: null,
      sessionId: "session-1",
    });
    await loadOlderPromise;

    const session = provider.currentState().sessionViews["session-1"];
    const timeline = session?.timeline ?? [];
    expect(session).toMatchObject({
      hasMoreHistory: false,
      historyLoading: false,
    });
    expect(
      timeline.some(
        (item) => item.type === "message" && item.kind === "user" && item.text === "stale older page",
      ),
    ).toBe(false);
    expect(
      timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "user" &&
          item.text === "rebuilt after switch back",
      ),
    ).toBe(true);
  });

  it("drops latest-history payloads whose sessionId does not match the requested session", async () => {
    const { provider } = buildProvider({
      getMessagesImpl: async () => ({
        hasMore: false,
        messages: [
          {
            id: "wrong-session-history",
            message: { content: "should be ignored", role: "assistant" },
            type: "message",
          },
        ],
        nextCursor: null,
        sessionId: "session-2",
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-history-session-guard",
      type: "ready",
    });

    expect(provider.currentState().sessionViews["session-1"]?.timeline).toEqual([]);
  });

  it("drops older-history payloads whose sessionId does not match the requested session", async () => {
    const { provider } = buildProvider({
      getMessagesImpl: async (_sessionId, params) => {
        if (params?.cursor === "cursor-1") {
          return {
            hasMore: false,
            messages: [
              {
                id: "wrong-older-history",
                message: { content: "should be ignored", role: "user" },
                type: "message",
              },
            ],
            nextCursor: null,
            sessionId: "session-2",
          };
        }
        return {
          hasMore: true,
          messages: [
            {
              id: "latest-history",
              message: { content: "latest history", role: "user" },
              type: "message",
            },
          ],
          nextCursor: "cursor-1",
          sessionId: "session-1",
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-older-session-guard",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "load-older-session-guard",
      type: "loadOlderHistory",
    });

    const session = provider.currentState().sessionViews["session-1"];
    expect(session).toMatchObject({
      hasMoreHistory: true,
      historyLoading: false,
    });
    expect(
      session?.timeline.some(
        (item) => item.type === "message" && item.kind === "user" && item.text === "latest history",
      ),
    ).toBe(true);
    expect(
      session?.timeline.some(
        (item) =>
          item.type === "message" && item.kind === "user" && item.text === "should be ignored",
      ),
    ).toBe(false);
  });

  it("keeps older loaded user history ahead of the live tail when switching back to a busy session", async () => {
    const olderMessages = Array.from({ length: 5 }, (_, index) => ({
      id: `older-user-${index + 1}`,
      message: { content: `ghost prompt ${index + 1}`, role: "user" },
      type: "message" as const,
    }));
    const recentMessages = Array.from({ length: 80 }, (_, index) => ({
      id: `recent-${index + 1}`,
      message: {
        content: `recent message ${index + 1}`,
        role: index % 2 === 0 ? "user" : "assistant",
      },
      type: "message" as const,
    }));
    const { messenger, provider } = buildProvider({
      getMessagesImpl: async (sessionId, params) => {
        if (sessionId === "session-2") {
          return {
            hasMore: false,
            messages: [],
            nextCursor: null,
            sessionId: "session-2",
          };
        }
        if (params?.cursor === "cursor-older-a") {
          return {
            hasMore: false,
            messages: olderMessages,
            nextCursor: null,
            sessionId: "session-1",
          };
        }
        return {
          hasMore: true,
          messages: recentMessages,
          nextCursor: "cursor-older-a",
          sessionId: "session-1",
        };
      },
      getStateImpl: async (sessionId) => ({
        busy: sessionId === "session-1",
        contextRatio: null,
        interrupted: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
      listSessionsImpl: async () => ({
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          { busy: true, isCurrent: true, sessionId: "session-1", updatedAt: 1 },
          { busy: false, isCurrent: false, sessionId: "session-2", updatedAt: 2 },
        ],
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-busy-switch-back-order",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "load-busy-switch-back-order",
      type: "loadOlderHistory",
    });

    messenger.emit({
      assistantMessageId: "live-tail-a",
      assistantMessageEvent: {
        delta: "still streaming",
        kind: "content_delta",
      },
      message: {},
      sessionId: "session-1",
      type: "message_update",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    await provider.dispatchTestIntent({
      data: { sessionId: "session-2" },
      messageId: "switch-busy-order-to-b",
      type: "switchSession",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "switch-busy-order-back-to-a",
      type: "switchSession",
    });

    const texts = (provider.currentState().sessionViews["session-1"]?.timeline ?? []).flatMap((item) =>
      item.type === "message" ? [item.text] : [],
    );
    const lastOlderIndex = texts.lastIndexOf("ghost prompt 5");
    const liveTailIndex = texts.lastIndexOf("still streaming");
    expect(texts.slice(0, 5)).toEqual(olderMessages.map((entry) => entry.message.content));
    expect(lastOlderIndex).toBeGreaterThanOrEqual(0);
    expect(liveTailIndex).toBe(texts.length - 1);
    expect(lastOlderIndex).toBeLessThan(liveTailIndex);
  });

  it("routes live events into their own session bucket while another session stays visible", async () => {
    const { messenger, provider } = buildProvider({
      getMessagesImpl: async (sessionId) => ({
        hasMore: false,
        messages: [],
        nextCursor: null,
        sessionId: sessionId ?? "session-1",
      }),
      getStateImpl: async (sessionId) => ({
        busy: false,
        contextRatio: null,
        interrupted: false,
        model: "gpt-5.4",
        planId: null,
        planPath: null,
        planState: "chat",
        sessionId: sessionId ?? "session-1",
        thinkingLevel: "high",
      }),
      listSessionsImpl: async () => ({
        activeSessionId: "session-1",
        scope: "disk" as const,
        sessions: [
          { busy: false, isCurrent: true, sessionId: "session-1", updatedAt: 1 },
          { busy: false, isCurrent: false, sessionId: "session-2", updatedAt: 2 },
        ],
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-cross-session-routing",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: { sessionId: "session-2" },
      messageId: "switch-cross-session-to-b",
      type: "switchSession",
    });

    messenger.emit({
      assistantMessageId: "assistant-live-a",
      assistantMessageEvent: {
        delta: "session A live event",
        kind: "content_delta",
      },
      message: {},
      sessionId: "session-1",
      type: "message_update",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(provider.currentState().activeSessionId).toBe("session-2");
    expect(
      provider.currentState().sessionViews["session-2"]?.timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "assistant" &&
          item.text === "session A live event",
      ),
    ).toBe(false);
    expect(
      provider.currentState().sessionViews["session-1"]?.timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "assistant" &&
          item.text === "session A live event",
      ),
    ).toBe(true);

    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "switch-cross-session-back-to-a",
      type: "switchSession",
    });

    expect(provider.currentState().activeSessionId).toBe("session-1");
    expect(
      provider.currentState().sessionViews["session-1"]?.timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "assistant" &&
          item.text === "session A live event",
      ),
    ).toBe(true);
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

  it("routes plan.todos events onto the active plan state", async () => {
    const { messenger, provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-plan-todos",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        action: "enter",
        sessionId: "session-1",
      },
      messageId: "plan-enter-todos",
      type: "setPlanMode",
    });

    messenger.emit({
      planId: "plan-1",
      sessionId: "session-1",
      todos: [
        { content: "Setup canvas", id: "t1", status: "pending" },
        { content: "Implement physics", id: "t2", status: "in_progress" },
        { content: "Verify in browser", id: "t3", status: "pending" },
      ],
      type: "plan.todos",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const session = provider.currentState().sessionViews["session-1"];
    expect(session.planFile).toMatchObject({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      state: "planning",
    });
    expect(session.planTodos).toHaveLength(3);
    expect(session.timeline.filter((item) => item.type === "plan")).toEqual([]);

    provider.dispose();
  });

  it("shows a VS Code error message when opening a file fails", async () => {
    const showErrorSpy = vi.spyOn(vscode.window, "showErrorMessage").mockResolvedValue(undefined);
    const { provider } = buildProvider({
      ideOverrides: {
        showFile: vi.fn().mockRejectedValue(new Error("boom")),
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-open-file-error",
      type: "ready",
    });

    await provider.dispatchTestIntent({
      data: { path: "/workspace/missing.ts" },
      messageId: "open-file-error",
      type: "openFile",
    });

    const timeline = provider.currentState().sessionViews["session-1"]?.timeline ?? [];
    expect(showErrorSpy).toHaveBeenCalledWith(
      expect.stringContaining("open file /workspace/missing.ts"),
    );
    expect(timeline.at(-1)).toMatchObject({
      kind: "assistant",
      text: "restored answer",
      type: "message",
    });

    showErrorSpy.mockRestore();
    provider.dispose();
  });

  it("shows a notice when answering an expired question", async () => {
    const { provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-stale-question",
      type: "ready",
    });

    await provider.dispatchTestIntent({
      data: {
        requestId: "missing-request",
        result: {
          answers: [],
          cancelled: false,
        },
        sessionId: "session-1",
      },
      messageId: "answer-stale-question",
      type: "answerQuestion",
    });

    const timeline = provider.currentState().sessionViews["session-1"]?.timeline ?? [];
    expect(timeline.at(-1)).toMatchObject({
      kind: "notice",
      text: "This question is no longer active. Please ask again if you still need it.",
      type: "message",
    });

    provider.dispose();
  });

  it("changes plan mode without reloading history", async () => {
    const { historyCalls, provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });
    expect(historyCalls).toHaveLength(1);

    await provider.dispatchTestIntent({
      data: {
        action: "enter",
        sessionId: "session-1",
      },
      messageId: "plan-enter-no-history",
      type: "setPlanMode",
    });

    expect(historyCalls).toHaveLength(1);
    expect(provider.currentState().sessionViews["session-1"]?.planState).toBe("planning");

    provider.dispose();
  });

  it("restores an active plan ref and context ratio from getState on ready", async () => {
    const { provider } = buildProvider({
      sessionState: {
        contextRatio: 0.42,
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    const session = provider.currentState().sessionViews["session-1"];
    expect(session).toMatchObject({
      contextRatio: 0.42,
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        planId: "plan-1",
        state: "executing",
      },
      planId: "plan-1",
      planState: "executing",
    });
    expect(session?.timeline.filter((item) => item.type === "plan")).toEqual([]);

    provider.dispose();
  });

  it("replays custom plan history while keeping current state truth", async () => {
    const { provider } = buildProvider({
      historyMessages: [
        {
          event: "plan.create",
          id: "hist-plan-create",
          path: "/workspace/plans/plan-1.plan.md",
          plan_id: "plan-1",
          state: "planning",
          type: "custom",
        },
        {
          event: "plan.review",
          id: "hist-plan-review",
          plan_id: "plan-1",
          summary: "looks good",
          type: "custom",
        },
        {
          event: "plan.verify",
          id: "hist-plan-verify",
          plan_id: "plan-1",
          type: "custom",
          verdict: "pass",
        },
      ],
      sessionState: {
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    const session = provider.currentState().sessionViews["session-1"];
    const notices = session?.timeline.filter(
      (item) => item.type === "message" && item.kind === "notice",
    );
    expect(session?.planFile).toMatchObject({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      state: "executing",
    });
    expect(session?.timeline.filter((item) => item.type === "plan")).toEqual([]);
    expect(notices).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ text: "Tomcat plan review: looks good" }),
        expect.objectContaining({ text: "Tomcat plan verify: pass" }),
      ]),
    );

    provider.dispose();
  });

  it("converges plan state from sequential transition events", async () => {
    const { messenger, provider } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    messenger.emit({
      sessionId: "session-1",
      type: "plan.enter",
    });
    messenger.emit({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      sessionId: "session-1",
      state: "executing",
      type: "plan.build",
    });
    messenger.emit({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      sessionId: "session-1",
      state: "pending",
      type: "plan.pending",
    });
    messenger.emit({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      sessionId: "session-1",
      state: "completed",
      type: "plan.complete",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const session = provider.currentState().sessionViews["session-1"];
    expect(session).toMatchObject({
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        state: "chat",
      },
      planId: null,
      planState: "chat",
    });
    expect(session?.timeline.filter((item) => item.type === "plan")).toEqual([]);

    provider.dispose();
  });

  it("projects the live code reviewer process into a single review row", async () => {
    const { messenger, provider } = buildProvider({
      sessionState: {
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-review-1",
      type: "ready",
    });

    messenger.emit({
      sessionId: "session-1",
      subagentType: "code_reviewer",
      type: "sub_agent_start",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    let review = provider
      .currentState()
      .sessionViews["session-1"]?.timeline.find((item) => item.type === "review");
    expect(review).toMatchObject({
      id: "review:plan-1",
      planId: "plan-1",
      status: "running",
      type: "review",
    });

    messenger.emit({
      findings: [{ area: "logic", note: "Missing null guard", severity: "concern" }],
      planId: "plan-1",
      rounds: 1,
      sessionId: "session-1",
      summary: "Fix the missing null guard before completing the plan.",
      type: "plan.code_review",
      verdict: "partial",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const reviews = provider
      .currentState()
      .sessionViews["session-1"]?.timeline.filter((item) => item.type === "review");
    expect(reviews).toHaveLength(1);
    review = reviews?.[0];
    expect(review).toMatchObject({
      findings: [{ area: "logic", note: "Missing null guard", severity: "concern" }],
      planId: "plan-1",
      rounds: 1,
      status: "done",
      summary: "Fix the missing null guard before completing the plan.",
      type: "review",
      verdict: "partial",
    });

    provider.dispose();
  });

  it("reconciles terminal plan events back to getState truth", async () => {
    const { messenger, provider, sessionState } = buildProvider({
      sessionState: {
        planId: "plan-1",
        planPath: "/workspace/plans/plan-1.plan.md",
        planState: "executing",
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-1",
      type: "ready",
    });

    sessionState.planId = null;
    sessionState.planPath = null;
    sessionState.planState = "chat";
    messenger.emit({
      path: "/workspace/plans/plan-1.plan.md",
      planId: "plan-1",
      sessionId: "session-1",
      state: "completed",
      type: "plan.complete",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const session = provider.currentState().sessionViews["session-1"];
    expect(session).toMatchObject({
      planFile: {
        path: "/workspace/plans/plan-1.plan.md",
        state: "chat",
      },
      planId: null,
      planState: "chat",
    });

    provider.dispose();
  });

  it("keeps interrupted sessions busy until agent_idle arrives, then returns to idle", async () => {
    const { messenger, provider, sessionState } = buildProvider({
      sessionState: {
        busy: true,
        interrupted: false,
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-interrupted",
      type: "ready",
    });
    expect(provider.currentState().sessionViews["session-1"]?.busy).toBe(true);

    sessionState.interrupted = true;
    messenger.emit({
      partialTextLen: 0,
      sessionId: "session-1",
      toolResultsCount: 1,
      type: "agent_interrupted",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    let session = provider.currentState().sessionViews["session-1"];
    expect(session?.busy).toBe(true);
    expect(session?.timeline).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "warn", text: "Tomcat turn interrupted", type: "message" }),
      ]),
    );

    messenger.emit({
      sessionId: "session-1",
      type: "agent_idle",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    session = provider.currentState().sessionViews["session-1"];
    expect(session?.busy).toBe(false);

    provider.dispose();
  });

  it("keeps busy true across stale agent_end get_state reconciles until agent_idle arrives", async () => {
    const { messenger, provider, sessionState } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-agent-idle-success",
      type: "ready",
    });
    messenger.emit({
      sessionId: "session-1",
      type: "agent_start",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(provider.currentState().sessionViews["session-1"]?.busy).toBe(true);

    sessionState.busy = true;
    messenger.emit({
      error: null,
      messages: [],
      sessionId: "session-1",
      type: "agent_end",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(provider.currentState().sessionViews["session-1"]?.busy).toBe(true);

    messenger.emit({
      sessionId: "session-1",
      type: "agent_idle",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const snapshot = provider.currentState();
    expect(snapshot.sessionViews["session-1"]?.busy).toBe(false);
    expect(snapshot.sessions.find((session) => session.sessionId === "session-1")?.busy).toBe(false);

    provider.dispose();
  });

  it("does not resurrect busy when a late agent_end arrives after agent_idle", async () => {
    const { messenger, provider, sessionState } = buildProvider();

    await provider.dispatchTestIntent({
      messageId: "ready-agent-idle-late-end",
      type: "ready",
    });
    messenger.emit({
      sessionId: "session-1",
      type: "agent_start",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    messenger.emit({
      sessionId: "session-1",
      type: "agent_idle",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(provider.currentState().sessionViews["session-1"]?.busy).toBe(false);

    sessionState.busy = true;
    messenger.emit({
      error: null,
      messages: [],
      sessionId: "session-1",
      type: "agent_end",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const snapshot = provider.currentState();
    expect(snapshot.sessionViews["session-1"]?.busy).toBe(false);
    expect(snapshot.sessions.find((session) => session.sessionId === "session-1")?.busy).toBe(false);

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

  it("surfaces prompt bridge timeouts on the inline failed user bubble", async () => {
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

    const timeoutMessage = provider.currentState().sessionViews["session-1"]?.timeline.find(
      (item) => item.type === "message" && item.kind === "user" && item.text === "will timeout",
    );
    expect(timeoutMessage).toMatchObject({
      deliveryError: expect.stringContaining("Tomcat bridge is not responding"),
      deliveryState: "failed",
      retryable: false,
      submitKind: "prompt",
      text: "will timeout",
      type: "message",
    });

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

  it("refreshes checkpoint markers and history after transcript-only restore", async () => {
    let restored = false;
    const restoreCalls: Array<{ checkpointId: string; revertFiles: boolean; sessionId: string }> = [];
    const { provider } = buildProvider({
      getMessagesImpl: async (sessionId) => ({
        messages: restored
          ? [
              {
                id: "hist-user-1",
                message: {
                  content: "first prompt",
                  role: "user",
                },
                type: "message",
              },
              {
                id: "hist-assistant-1",
                message: {
                  content: "first reply",
                  role: "assistant",
                },
                type: "message",
              },
              {
                id: "hist-user-2",
                message: {
                  content: "second prompt",
                  role: "user",
                  superseded: true,
                },
                type: "message",
              },
              {
                id: "hist-assistant-2",
                message: {
                  content: "second reply",
                  role: "assistant",
                  superseded: true,
                },
                type: "message",
              },
              {
                customType: "checkpoint.restore",
                id: "restore-entry-1",
                type: "custom",
              },
            ]
          : [
              {
                id: "hist-user-1",
                message: {
                  content: "first prompt",
                  role: "user",
                },
                type: "message",
              },
              {
                id: "hist-assistant-1",
                message: {
                  content: "first reply",
                  role: "assistant",
                },
                type: "message",
              },
              {
                id: "hist-user-2",
                message: {
                  content: "second prompt",
                  role: "user",
                },
                type: "message",
              },
              {
                id: "hist-assistant-2",
                message: {
                  content: "second reply",
                  role: "assistant",
                },
                type: "message",
              },
            ],
        sessionId: sessionId ?? "session-1",
        upToSeq: null,
      }),
      listCheckpointsImpl: async (sessionId) => ({
        checkpoints: restored
          ? []
          : [
              {
                changedFiles: ["src/app.ts"],
                createdAt: "2026-07-12T12:00:00Z",
                id: "ck-1",
                kind: "turn_end",
                messageAnchor: "hist-assistant-1",
              },
            ],
        sessionId: sessionId ?? "session-1",
      }),
      restoreCheckpointImpl: async (sessionId, checkpointId, revertFiles) => {
        restoreCalls.push({ checkpointId, revertFiles, sessionId });
        restored = true;
        return {
          changedPaths: ["src/app.ts"],
          checkpointId,
          createdAt: "2026-07-12T12:00:00Z",
          dryRun: false,
          kind: "turn_end",
          restoredPaths: [],
          revertFiles,
          sessionId,
          transcriptTruncated: true,
          warnings: [],
        };
      },
    });

    await provider.dispatchTestIntent({
      messageId: "ready-restore-checkpoint",
      type: "ready",
    });

    expect(provider.currentState().sessionViews["session-1"]?.timeline.map((item) => item.id)).toEqual([
      "hist-user-1",
      "hist-assistant-1",
      "hist-user-2",
      "hist-assistant-2",
    ]);
    expect(provider.currentState().sessionViews["session-1"]?.checkpoints).toEqual([
      {
        changedFiles: ["src/app.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        label: null,
        messageAnchor: "hist-assistant-1",
      },
    ]);

    await provider.dispatchTestIntent({
      data: {
        checkpointId: "ck-1",
        revertFiles: false,
        sessionId: "session-1",
      },
      messageId: "restore-checkpoint-1",
      type: "restoreCheckpoint",
    });

    expect(restoreCalls).toEqual([
      {
        checkpointId: "ck-1",
        revertFiles: false,
        sessionId: "session-1",
      },
    ]);
    expect(provider.currentState().sessionViews["session-1"]?.timeline.map((item) => item.id)).toEqual([
      "hist-user-1",
      "hist-assistant-1",
    ]);
    expect(provider.currentState().sessionViews["session-1"]?.checkpoints).toEqual([]);

    provider.dispose();
  });

  it("keeps the latest live turn visible when checkpoint refresh runs against stale history", async () => {
    const { messenger, provider } = buildProvider({
      getMessagesImpl: async (sessionId) => ({
        messages: [
          {
            id: "hist-user-1",
            message: {
              content: "first prompt",
              role: "user",
            },
            type: "message",
          },
          {
            id: "hist-assistant-1",
            message: {
              content: "first reply",
              role: "assistant",
            },
            type: "message",
          },
        ],
        sessionId: sessionId ?? "session-1",
        upToSeq: null,
      }),
      listCheckpointsImpl: async (sessionId) => ({
        checkpoints: [
          {
            changedFiles: ["src/app.ts"],
            createdAt: "2026-07-12T12:00:00Z",
            id: "ck-1",
            kind: "turn_end",
            messageAnchor: "hist-assistant-1",
          },
        ],
        sessionId: sessionId ?? "session-1",
      }),
    });

    await provider.dispatchTestIntent({
      messageId: "ready-live-checkpoint-refresh",
      type: "ready",
    });
    await provider.dispatchTestIntent({
      data: {
        sessionId: "session-1",
        text: "latest prompt",
      },
      messageId: "prompt-live-checkpoint-refresh",
      type: "prompt",
    });

    messenger.emit({
      assistantMessageEvent: { delta: "latest answer", kind: "content_delta" },
      assistantMessageId: "assistant-live-1",
      message: {},
      sessionId: "session-1",
      type: "message_update",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    messenger.emit({
      assistantMessageId: "assistant-live-1",
      message: {},
      sessionId: "session-1",
      toolCallIds: [],
      toolResults: [],
      turnIndex: 1,
      type: "turn_end",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    const afterTurnEnd = provider.currentState().sessionViews["session-1"];
    const textsAfterTurnEnd = (afterTurnEnd?.timeline ?? []).flatMap((item) =>
      item.type === "message" ? [item.text] : [],
    );
    expect(textsAfterTurnEnd).toContain("first prompt");
    expect(textsAfterTurnEnd).toContain("first reply");
    expect(textsAfterTurnEnd).toContain("latest prompt");
    expect(textsAfterTurnEnd).toContain("latest answer");
    expect((afterTurnEnd?.timeline ?? []).every((item) => item.type !== "checkpoint")).toBe(true);
    expect(afterTurnEnd?.checkpoints).toEqual([
      {
        changedFiles: ["src/app.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        label: null,
        messageAnchor: "hist-assistant-1",
      },
    ]);

    const beforeListCheckpoints = (afterTurnEnd?.timeline ?? []).map((item) => item.id);

    await provider.dispatchTestIntent({
      data: { sessionId: "session-1" },
      messageId: "list-checkpoints-live-checkpoint-refresh",
      type: "listCheckpoints",
    });

    expect(provider.currentState().sessionViews["session-1"]?.timeline.map((item) => item.id)).toEqual(
      beforeListCheckpoints,
    );

    provider.dispose();
  });
});
