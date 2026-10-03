import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import {
  TomcatWebviewViewProvider,
  buildAttachmentOpenDialogOptions,
  classifyPickedUri,
  parseModelCatalog,
  parsePlanFrontmatter,
  readPlanMetadata,
} from "../provider";
import type { HostToWebviewFrame } from "../protocol";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      registerDirectory(dirPath: string): void;
      setErrorMessageHandler(handler: ((message: string, items: string[]) => string | undefined) | undefined): void;
      setWarningMessageHandler(
        handler:
          | ((message: string, items: string[], options?: { detail?: string; modal?: boolean }) => string | undefined)
          | undefined,
      ): void;
      registerFile(filePath: string, text: string): void;
      reset(): void;
      setConfiguration(key: string, value: unknown): void;
    };
  }
).__testing;

describe("plan metadata helpers", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(
      tempDirs.map(async (dir) => {
        await fs.rm(dir, { force: true, recursive: true });
      }),
    );
    tempDirs.length = 0;
  });

  it("parses title and overview from plan frontmatter", () => {
    const parsed = parsePlanFrontmatter(`---
name: Demo Plan UI
overview: Render the transcript UI with plan metadata.
todos:
  - id: one
---
# body
`);

    expect(parsed).toEqual({
      overview: "Render the transcript UI with plan metadata.",
      title: "Demo Plan UI",
    });
  });

  it("falls back to goal as title when name/title are absent", () => {
    const parsed = parsePlanFrontmatter(`---
goal: 在 test-stuff/ 下创建经典世嘉 OutRun 风格赛车网页游戏
draft: ...
---
# body
`);

    expect(parsed).toEqual({
      title: "在 test-stuff/ 下创建经典世嘉 OutRun 风格赛车网页游戏",
    });
  });

  it("truncates a long goal to the first line and 96 chars", () => {
    const longGoal = "目标".repeat(60);
    const parsed = parsePlanFrontmatter(`---
goal: ${longGoal}
---
`);
    expect(parsed.title).toBeDefined();
    expect(parsed.title!.length).toBeLessThanOrEqual(96);
    expect(parsed.title!.endsWith("...")).toBe(true);
  });

  it("prefers explicit title/name over goal", () => {
    const byTitle = parsePlanFrontmatter(`---
title: Explicit Title
goal: some goal
---
`);
    expect(byTitle.title).toBe("Explicit Title");

    const byName = parsePlanFrontmatter(`---
name: Named Plan
goal: some goal
---
`);
    expect(byName.title).toBe("Named Plan");
  });

  it("returns empty metadata when there is no frontmatter", () => {
    expect(parsePlanFrontmatter("# just a body\nno frontmatter here")).toEqual({});
  });

  it("reads metadata from disk and refreshes the cache when the file changes", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-plan-metadata-"));
    tempDirs.push(dir);
    const filePath = path.join(dir, "demo.plan.md");
    const cache = new Map<string, { mtimeMs: number; overview?: string; title?: string }>();

    await fs.writeFile(
      filePath,
      `---
name: First Title
overview: First overview.
---
`,
      "utf8",
    );

    const first = await readPlanMetadata(filePath, cache);
    expect(first).toEqual({
      overview: "First overview.",
      title: "First Title",
    });

    await new Promise((resolve) => setTimeout(resolve, 20));
    await fs.writeFile(
      filePath,
      `---
name: Updated Title
overview: Updated overview.
---
`,
      "utf8",
    );

    const second = await readPlanMetadata(filePath, cache);
    expect(second).toEqual({
      overview: "Updated overview.",
      title: "Updated Title",
    });
  });

  it("expands ~ in the plan path before reading from disk", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-plan-home-"));
    tempDirs.push(dir);
    const previousHome = process.env.HOME;
    process.env.HOME = dir;
    try {
      const planPath = path.join(dir, "demo.plan.md");
      await fs.writeFile(
        planPath,
        `---
goal: Home-expanded plan
---
`,
        "utf8",
      );

      const cache = new Map<string, { mtimeMs: number; overview?: string; title?: string }>();
      const metadata = await readPlanMetadata("~/demo.plan.md", cache);
      expect(metadata).toEqual({ title: "Home-expanded plan" });
    } finally {
      process.env.HOME = previousHome;
    }
  });
});

describe("attachment picker options", () => {
  it("allows any file or folder and updates the action label", () => {
    expect(buildAttachmentOpenDialogOptions()).toEqual({
      canSelectFiles: true,
      canSelectFolders: true,
      canSelectMany: true,
      openLabel: "Add to Tomcat",
    });
  });
});

describe("picked uri classification", () => {
  it("routes directories to references and images/pdf to attachments", async () => {
    __testing.reset();
    __testing.registerDirectory("/workspace/src/folder");
    __testing.registerFile("/workspace/assets/mockup.png", "png");
    __testing.registerFile("/workspace/specs/notes.pdf", "%PDF");
    __testing.registerFile("/workspace/src/app.ts", "export const answer = 42;\n");
    __testing.registerFile("/workspace/tmp/blob.bin", "raw");

    await expect(classifyPickedUri(vscode.Uri.file("/workspace/src/folder"))).resolves.toBe("reference");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/assets/mockup.png"))).resolves.toBe("attachment");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/specs/notes.pdf"))).resolves.toBe("attachment");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/src/app.ts"))).resolves.toBe("reference");
    await expect(classifyPickedUri(vscode.Uri.file("/workspace/tmp/blob.bin"))).resolves.toBe("reference");
  });

  it("commits a mixed picker selection through one draft lane", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
    const host = provider as any;
    const firstReference = {
      kind: "file",
      path: "/workspace/src/first.ts",
      type: "reference",
    };
    const secondReference = {
      kind: "file",
      path: "/workspace/src/second.ts",
      type: "reference",
    };
    const attachment = {
      blobSha: "a".repeat(64),
      bytes: 12,
      filename: "diagram.png",
      id: "attachment-1",
      kind: "image",
      mimeType: "image/png",
    };
    const run = vi.spyOn(host.draftCoordinator, "run");
    vi.spyOn(host, "resolvePickedUri")
      .mockResolvedValueOnce({ kind: "reference", reference: firstReference })
      .mockResolvedValueOnce({ kind: "attachment", upload: { filename: "diagram.png" } })
      .mockResolvedValueOnce({ kind: "reference", reference: secondReference });
    vi.spyOn(host, "ingestUploads").mockImplementation(async (...args: unknown[]) => {
      const sessionId = args[0] as string;
      await host.saveAttachmentsToDraft(sessionId, [attachment]);
      return [attachment];
    });
    vi.spyOn(host, "postInsertedReference").mockResolvedValue(undefined);
    vi.spyOn(host, "reportAttachmentOutcome").mockResolvedValue(undefined);

    await host.ingestPickedUris("s1", [
      vscode.Uri.file("/workspace/src/first.ts"),
      vscode.Uri.file("/workspace/assets/diagram.png"),
      vscode.Uri.file("/workspace/src/second.ts"),
    ]);

    expect(run).toHaveBeenCalledTimes(1);
    expect(host.draftStore.peek("s1")).toMatchObject({
      attachments: [attachment],
      segments: [firstReference, secondReference],
    });
    provider.dispose();
  });
});

describe("draft fork delivery", () => {
  function attachFailingWebview(
    provider: TomcatWebviewViewProvider,
    postMessage: ReturnType<typeof vi.fn>,
  ): void {
    provider.resolveWebviewView({
      onDidChangeVisibility: () => new vscode.Disposable(() => undefined),
      show() {},
      visible: true,
      webview: {
        asWebviewUri: (uri: vscode.Uri) => uri,
        cspSource: "vscode-test-webview",
        html: "",
        onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
        options: {},
        postMessage,
      },
    } as unknown as vscode.WebviewView);
  }

  it("retries a failed fork-result hand-off three times then rejects and clears the operation", async () => {
    vi.useFakeTimers();
    try {
      const postMessage = vi.fn().mockRejectedValue(new Error("bridge down"));
      const provider = new TomcatWebviewViewProvider({
        extensionUri: vscode.Uri.file("/workspace/extension"),
        getDefaultCwd: () => "/workspace",
        ide: {} as never,
        initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
        messenger: { onEvent: () => ({ dispose() {} }) } as never,
        sessionRouter: {} as never,
      });
      attachFailingWebview(provider, postMessage);
      const host = provider as any;
      const resolve = vi.fn();
      const reject = vi.fn();
      const operation = {
        captureAccepted: false,
        cwd: null,
        operationId: "fork-delivery-failure",
        promise: Promise.resolve("unused"),
        reject,
        resolve,
        sourceSessionId: "s1",
      };
      host.pendingDraftForks.set(operation.operationId, operation);
      host.pendingDraftForkBySource.set(operation.sourceSessionId, operation);
      vi.spyOn(host, "executeDraftFork").mockResolvedValue("target-session");

      const capture = host.handleDraftForkCapture({
        cwd: null,
        operationId: operation.operationId,
        segments: [],
        sourceSessionId: "s1",
        text: "",
      });
      await vi.runAllTimersAsync();
      await capture;

      expect(postMessage).toHaveBeenCalledTimes(3);
      expect(resolve).not.toHaveBeenCalled();
      expect(reject).toHaveBeenCalledWith(expect.objectContaining({ message: "bridge down" }));
      expect(host.pendingDraftForks.size).toBe(0);
      expect(host.pendingDraftForkBySource.size).toBe(0);
      provider.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps ordinary event delivery best effort when the bridge rejects", async () => {
    const postMessage = vi.fn().mockRejectedValue(new Error("bridge down"));
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
    attachFailingWebview(provider, postMessage);

    await expect((provider as any).postEvent({
      data: { hasErrors: false, message: "attached" },
      type: "attachmentFeedback",
    })).resolves.toBeUndefined();
    await Promise.resolve();
    expect(postMessage).toHaveBeenCalledTimes(1);
    provider.dispose();
  });

  it("rejects and clears pending draft forks when a test webview reloads", () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
    const host = provider as any;
    const reject = vi.fn();
    const operation = {
      captureAccepted: false,
      cwd: null,
      operationId: "reload-fork",
      promise: Promise.resolve("unused"),
      reject,
      resolve: vi.fn(),
      sourceSessionId: "s1",
    };
    host.pendingDraftForks.set(operation.operationId, operation);
    host.pendingDraftForkBySource.set(operation.sourceSessionId, operation);

    provider.resetForTestReload();

    expect(reject).toHaveBeenCalledOnce();
    expect(reject.mock.calls[0]?.[0]).toMatchObject({
      message: "Tomcat webview reloaded before draft fork capture completed",
    });
    expect(host.pendingDraftForks.size).toBe(0);
    expect(host.pendingDraftForkBySource.size).toBe(0);
    provider.dispose();
  });
});

describe("model catalog parsing", () => {
  it("retains per-model metadata and defaults speeds for legacy catalogs", () => {
    expect(
      parseModelCatalog({
        models: [
          {
            capabilities: {
              reasoning: true,
            },
            contextWindow: 400000,
            contextWindowOptions: [400000, 1000000],
            description: "Fast reasoning model",
            id: "deepseek-v4-flash",
            keyPresent: true,
            selectedContextWindow: 1000000,
            selectedReasoningLevel: "max",
            supportedReasoningLevels: ["high", "max"],
          },
          {
            capabilities: ["vision", "files"],
            id: "gpt-5.4",
            keyPresent: true,
            supportedReasoningLevels: ["low", "medium", "high", "xhigh"],
          },
          {
            capabilities: null,
            id: "text-only",
            keyPresent: true,
          },
          {
            capabilities: {
              tools: true,
            },
            id: "missing-key",
            keyPresent: false,
          },
        ],
      }),
    ).toEqual({
      capabilities: {
        "deepseek-v4-flash": ["reasoning"],
        "gpt-5.4": ["vision", "files"],
        "text-only": [],
      },
      ids: ["deepseek-v4-flash", "gpt-5.4", "text-only"],
      modelDetails: {
        "deepseek-v4-flash": {
          capabilities: ["reasoning"],
          contextWindow: 400000,
          contextWindowOptions: [400000, 1000000],
          description: "Fast reasoning model",
          id: "deepseek-v4-flash",
          modelName: null,
          selectedContextWindow: 1000000,
          selectedReasoningLevel: "max",
          selectedSpeed: null,
          supportedSpeeds: [],
          supportedReasoningLevels: ["high", "max"],
        },
        "gpt-5.4": {
          capabilities: ["vision", "files"],
          contextWindowOptions: [],
          description: null,
          id: "gpt-5.4",
          modelName: null,
          selectedReasoningLevel: null,
          selectedSpeed: null,
          supportedSpeeds: [],
          supportedReasoningLevels: ["low", "medium", "high", "xhigh"],
        },
        "text-only": {
          capabilities: [],
          contextWindowOptions: [],
          description: null,
          id: "text-only",
          modelName: null,
          selectedReasoningLevel: null,
          selectedSpeed: null,
          supportedSpeeds: [],
          supportedReasoningLevels: [],
        },
      },
      reasoningLevels: {
        "deepseek-v4-flash": ["high", "max"],
        "gpt-5.4": ["low", "medium", "high", "xhigh"],
        "text-only": [],
      },
    });
  });
});

describe("error-turn recovery", () => {
  function createRecoveryProvider(
    retry: ReturnType<typeof vi.fn>,
    resume: ReturnType<typeof vi.fn>,
  ) {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1", capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: { retry, resume } as never,
    });
  }

  function seedRetryableError(provider: TomcatWebviewViewProvider): void {
    const host = provider as any;
    host.stateStore.setActiveSession("s1");
    host.stateStore.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "keep this exact prompt",
            role: "user",
            superseded: true,
            turn_failed: true,
          },
          type: "message",
        },
        { detail: "failed", id: "error-1", summary: "failed", type: "error" },
      ],
      sessionId: "s1",
    });
    vi.spyOn(host, "ensureInitialized").mockResolvedValue({ sessionId: "s1" });
    vi.spyOn(host, "ensureWebviewSession").mockResolvedValue("s1");
    vi.spyOn(host, "refreshSessionState").mockResolvedValue(undefined);
    vi.spyOn(host, "refreshSessions").mockResolvedValue(undefined);
    vi.spyOn(host, "postState").mockResolvedValue(undefined);
  }

  it("keeps the failed chapter visible while durable Retry starts", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const resume = vi.fn().mockResolvedValue(undefined);
    const provider = createRecoveryProvider(retry, resume);
    seedRetryableError(provider);
    const host = provider as any;
    const sendUserMessage = vi.spyOn(host, "sendUserMessage");

    await host.handleIntent({
      data: { action: "retry", errorId: "error-1", sessionId: "s1" },
      messageId: "retry-error-1",
      type: "recoverErrorTurn",
    });

    expect(retry).toHaveBeenCalledWith("s1", "user-1");
    expect(resume).not.toHaveBeenCalled();
    expect(sendUserMessage).not.toHaveBeenCalled();
    const failedChapter = host.currentState().sessionViews.s1.timeline.filter(
      (item: { kind?: string; type: string }) => item.type === "message" && item.kind === "user",
    );
    expect(failedChapter).toHaveLength(1);
    expect(failedChapter[0]).toMatchObject({ abandoned: true, id: "user-1" });
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "error-1",
      ),
    ).toMatchObject({ id: "error-1", kind: "error" });
    provider.dispose();
  });

  it("restores the same card when Retry is rejected", async () => {
    const retry = vi.fn().mockRejectedValue(new Error("retry_target_stale"));
    const resume = vi.fn().mockRejectedValue(new Error("bridge unavailable"));
    const provider = createRecoveryProvider(retry, resume);
    seedRetryableError(provider);
    const host = provider as any;
    const warnings: string[] = [];
    __testing.setWarningMessageHandler((message) => {
      warnings.push(message);
      return undefined;
    });

    await host.handleIntent({
      data: { action: "retry", errorId: "error-1", sessionId: "s1" },
      messageId: "retry-error-1",
      type: "recoverErrorTurn",
    });

    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "error-1",
      ),
    ).toMatchObject({ recoveryAction: "retry" });
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "user-1",
      ),
    ).toMatchObject({ kind: "user", text: "keep this exact prompt" });
    expect(warnings).toEqual([]);
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "error-1",
      ),
    ).toMatchObject({
      recoveryError: "这张错误卡已经过期，无法重试。请刷新会话后重新输入。",
    });
    provider.dispose();
    __testing.setWarningMessageHandler(undefined);
  });

  it("keeps Resume on the no-input recovery route", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const resume = vi.fn().mockResolvedValue(undefined);
    const provider = createRecoveryProvider(retry, resume);
    const host = provider as any;
    host.stateStore.setActiveSession("s1");
    host.stateStore.hydrateHistory("s1", {
      messages: [
        { id: "user-1", message: { content: "inspect", role: "user" }, type: "message" },
        {
          id: "assistant-1",
          message: {
            content: null,
            role: "assistant",
            tool_calls: [{ id: "read-1", name: "read" }],
          },
          type: "message",
        },
        {
          id: "tool-1",
          message: { content: "contents", role: "tool", tool_call_id: "read-1" },
          type: "message",
        },
        { detail: "failed", id: "error-1", summary: "failed", type: "error" },
      ],
      sessionId: "s1",
    });
    vi.spyOn(host, "ensureInitialized").mockResolvedValue({ sessionId: "s1" });
    vi.spyOn(host, "ensureWebviewSession").mockResolvedValue("s1");
    vi.spyOn(host, "refreshSessionState").mockResolvedValue(undefined);
    vi.spyOn(host, "refreshSessions").mockResolvedValue(undefined);
    vi.spyOn(host, "postState").mockResolvedValue(undefined);

    await host.handleIntent({
      data: { action: "resume", errorId: "error-1", sessionId: "s1" },
      messageId: "resume-error-1",
      type: "recoverErrorTurn",
    });

    expect(resume).toHaveBeenCalledWith("s1");
    expect(retry).not.toHaveBeenCalled();
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "error-1",
      ),
    ).toMatchObject({ id: "error-1", kind: "error" });
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "user-1",
      ),
    ).toMatchObject({ kind: "user", text: "inspect" });
    provider.dispose();
  });

  it("distinguishes an unavailable bridge from a stale recovery anchor", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const resume = vi.fn().mockRejectedValue(new Error("bridge unavailable"));
    const provider = createRecoveryProvider(retry, resume);
    const host = provider as any;
    host.stateStore.setActiveSession("s1");
    host.stateStore.hydrateHistory("s1", {
      messages: [
        { id: "user-1", message: { content: "inspect", role: "user" }, type: "message" },
        {
          id: "assistant-1",
          message: {
            content: null,
            role: "assistant",
            tool_calls: [{ id: "read-1", name: "read" }],
          },
          type: "message",
        },
        {
          id: "tool-1",
          message: { content: "contents", role: "tool", tool_call_id: "read-1" },
          type: "message",
        },
        { detail: "failed", id: "error-1", summary: "failed", type: "error" },
      ],
      sessionId: "s1",
    });
    vi.spyOn(host, "ensureInitialized").mockResolvedValue({ sessionId: "s1" });
    vi.spyOn(host, "ensureWebviewSession").mockResolvedValue("s1");
    vi.spyOn(host, "postState").mockResolvedValue(undefined);
    const warnings: string[] = [];
    __testing.setWarningMessageHandler((message) => {
      warnings.push(message);
      return undefined;
    });

    await host.handleIntent({
      data: { action: "resume", errorId: "error-1", sessionId: "s1" },
      messageId: "resume-error-1",
      type: "recoverErrorTurn",
    });

    expect(resume).toHaveBeenCalledWith("s1");
    expect(warnings).toEqual([]);
    expect(
      host.currentState().sessionViews.s1.timeline.find(
        (item: { id?: string; type: string }) => item.type === "message" && item.id === "error-1",
      ),
    ).toMatchObject({
      recoveryAction: "resume",
      recoveryError: "Unable to recover this turn: bridge unavailable",
    });
    provider.dispose();
    __testing.setWarningMessageHandler(undefined);
  });
});

describe("thinking level intent handling", () => {
  it("routes max through handleWebviewMessage and refreshes session state", async () => {
    const sendSetThinkingLevel = vi.fn().mockResolvedValue({ success: true });
    const refreshModels = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        sendSetThinkingLevel,
      } as never,
      sessionRouter: {
        getState: vi.fn().mockResolvedValue({
          busy: false,
          model: "claude-4.6-sonnet",
          sessionId: "s1",
          thinkingLevel: "max",
        }),
      } as never,
    });
    vi.spyOn(provider as any, "ensureInitialized").mockResolvedValue(undefined);
    vi.spyOn(provider as any, "ensureWebviewSession").mockResolvedValue("s1");
    vi.spyOn(provider as any, "refreshModels").mockImplementation(refreshModels);
    vi.spyOn(provider as any, "postState").mockResolvedValue(undefined);

    await (
      provider as unknown as {
        handleWebviewMessage(message: unknown): Promise<void>;
      }
    ).handleWebviewMessage({
      data: {
        level: "max",
        modelId: "claude-4.6-sonnet",
        sessionId: "s1",
      },
      messageId: "thinking-max",
      type: "setThinkingLevel",
    });

    expect(sendSetThinkingLevel).toHaveBeenCalledWith("s1", "claude-4.6-sonnet", "max");
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(provider.currentState().sessionViews.s1).toMatchObject({
      model: "claude-4.6-sonnet",
      thinkingLevel: "max",
    });

    provider.dispose();
  });
});

describe("webview html asset resolution", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(
      tempDirs.map(async (dir) => {
        await fs.rm(dir, { force: true, recursive: true });
      }),
    );
    tempDirs.length = 0;
  });

  async function createExtensionRoot(files: Record<string, string>): Promise<vscode.Uri> {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-webview-assets-"));
    tempDirs.push(dir);
    await Promise.all(
      Object.entries(files).map(async ([relativePath, contents]) => {
        const filePath = path.join(dir, relativePath);
        await fs.mkdir(path.dirname(filePath), { recursive: true });
        await fs.writeFile(filePath, contents, "utf8");
      }),
    );
    return vscode.Uri.file(dir);
  }

  function createWebview(): vscode.Webview {
    return {
      asWebviewUri(uri: vscode.Uri) {
        return uri;
      },
      cspSource: "vscode-test-webview",
    } as unknown as vscode.Webview;
  }

  it("links the built stylesheet when gui dist ships styles.css", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/index.js": "console.log('index');",
      "gui/dist/styles.css": "body { color: red; }",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri,
      getDefaultCwd: () => undefined,
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    const html = (
      provider as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain('rel="stylesheet"');
    expect(html).toContain('class="tc-chat-webview"');
    expect(html).toContain('--tc-controls-inset:10px;--tc-content-inset:15px');
    __testing.setConfiguration("tomcat.layout.controlsInset",4);
    __testing.setConfiguration("tomcat.layout.contentInset",24);
    expect((provider as any).renderHtml(createWebview())).toContain('--tc-controls-inset:4px;--tc-content-inset:24px');
    __testing.setConfiguration("tomcat.layout.controlsInset",undefined);
    __testing.setConfiguration("tomcat.layout.contentInset",undefined);
    expect(html).toContain("styles.css");
    provider.dispose();
  });

  it("carries every stylesheet the built index.html declares (codicon.css guard)", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/index.html": `<!doctype html><html><head>
        <script type="module" crossorigin src="./index.js"></script>
        <link rel="stylesheet" crossorigin href="./styles.css">
        <link rel="stylesheet" crossorigin href="./codicon.css">
      </head><body><div id="root"></div></body></html>`,
      "gui/dist/index.js": "console.log('index');",
      "gui/dist/styles.css": "body { color: red; }",
      "gui/dist/codicon.css": "@font-face { font-family: codicon; }",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri,
      getDefaultCwd: () => undefined,
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    const html = (
      provider as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    // The icon font stylesheet must be linked, or every codicon renders blank.
    expect(html).toContain("styles.css");
    expect(html).toContain("codicon.css");
    provider.dispose();
  });

  it("allows dynamic import chunks and mermaid inline styles in the chat webview CSP", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/index.js": "console.log('index');",
      "gui/dist/styles.css": "body { color: red; }",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri,
      getDefaultCwd: () => undefined,
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    const html = (
      provider as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain("style-src vscode-test-webview 'unsafe-inline';");
    expect(html).toContain("script-src 'nonce-");
    expect(html).toContain("'strict-dynamic';");
    provider.dispose();
  });
});

function buildSearchProvider(): {
  postedFrames: HostToWebviewFrame[];
  provider: TomcatWebviewViewProvider;
} {
  const postedFrames: HostToWebviewFrame[] = [];
  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/workspace/extension"),
    getDefaultCwd: () => "/workspace",
    ide: {} as never,
    initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
    messenger: {
      onEvent: () => ({ dispose() {} }),
    } as never,
    sessionRouter: {} as never,
  });

  provider.resolveWebviewView({
    onDidChangeVisibility: () => new vscode.Disposable(() => undefined),
    show() {},
    visible: true,
    webview: {
      asWebviewUri(uri: vscode.Uri) {
        return uri;
      },
      cspSource: "vscode-test-webview",
      html: "",
      onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
      options: {},
      postMessage: async (frame: HostToWebviewFrame) => {
        postedFrames.push(frame);
        return true;
      },
    },
  } as unknown as vscode.WebviewView);

  return { postedFrames, provider };
}

describe("context search intent handling", () => {
  it("routes searchContext intents into contextSearchResult events", async () => {
    __testing.reset();
    __testing.registerFile("/workspace/src/app.ts", "export const app = true;\n");
    const { postedFrames, provider } = buildSearchProvider();

    await provider.dispatchTestIntent({
      data: {
        query: "app",
        requestId: "req-1",
        sessionId: "session-1",
      },
      messageId: "search-1",
      type: "searchContext",
    });

    expect(postedFrames.at(-1)).toEqual({
      channel: "event",
      content: {
        matches: [
          {
            description: "src",
            reference: {
              kind: "file",
              label: "app.ts",
              path: "src/app.ts",
              type: "reference",
            },
          },
        ],
        query: "app",
        requestId: "req-1",
        sessionId: "session-1",
        truncated: false,
        type: "contextSearchResult",
        workspaceAvailable: true,
      },
      messageId: expect.any(String),
    });

    provider.dispose();
  });

  it("cancels the previous search when a new query arrives", async () => {
    __testing.reset();
    __testing.registerFile("/workspace/src/new.ts", "export const next = true;\n");
    const { postedFrames, provider } = buildSearchProvider();
    let firstCancelled = false;
    const findFilesSpy = vi
      .spyOn(vscode.workspace, "findFiles")
      .mockImplementationOnce(
        async (_include, _exclude, _maxResults, token) =>
          new Promise((resolve) => {
            token?.onCancellationRequested(() => {
              firstCancelled = true;
              resolve([]);
            });
          }),
      )
      .mockResolvedValueOnce([vscode.Uri.file("/workspace/src/new.ts")]);

    const firstRequest = provider.dispatchTestIntent({
      data: {
        query: "old",
        requestId: "req-old",
        sessionId: "session-1",
      },
      messageId: "search-old",
      type: "searchContext",
    });
    const secondRequest = provider.dispatchTestIntent({
      data: {
        query: "new",
        requestId: "req-new",
        sessionId: "session-1",
      },
      messageId: "search-new",
      type: "searchContext",
    });

    await Promise.all([firstRequest, secondRequest]);

    expect(firstCancelled).toBe(true);
    const resultsByRequestId = new Map(
      postedFrames.map((frame) => [
        (frame.content as { requestId?: string }).requestId,
        frame.content,
      ]),
    );
    expect(resultsByRequestId.get("req-old")).toEqual(
      expect.objectContaining({
        matches: [],
        query: "old",
        requestId: "req-old",
        truncated: false,
        type: "contextSearchResult",
      }),
    );
    expect(resultsByRequestId.get("req-new")).toEqual(
      expect.objectContaining({
        matches: [
          {
            description: "src",
            reference: {
              kind: "file",
              label: "new.ts",
              path: "src/new.ts",
              type: "reference",
            },
          },
        ],
        query: "new",
        requestId: "req-new",
        type: "contextSearchResult",
      }),
    );

    findFilesSpy.mockRestore();
    provider.dispose();
  });

  it("returns an empty result when no workspace folder is open", async () => {
    __testing.reset();
    const workspace = vscode.workspace as typeof vscode.workspace & {
      workspaceFolders: Array<{ uri: vscode.Uri }>;
    };
    workspace.workspaceFolders = [];
    const { postedFrames, provider } = buildSearchProvider();

    await provider.dispatchTestIntent({
      data: {
        query: "app",
        requestId: "req-noworkspace",
      },
      messageId: "search-noworkspace",
      type: "searchContext",
    });

    expect(postedFrames.at(-1)?.content).toEqual({
      matches: [],
      query: "app",
      requestId: "req-noworkspace",
      sessionId: null,
      truncated: false,
      type: "contextSearchResult",
      workspaceAvailable: false,
    });

    provider.dispose();
  });

  it("swallows search errors and responds with an empty result", async () => {
    __testing.reset();
    const { postedFrames, provider } = buildSearchProvider();
    const findFilesSpy = vi
      .spyOn(vscode.workspace, "findFiles")
      .mockRejectedValueOnce(new Error("boom"));
    const consoleErrorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(
      provider.dispatchTestIntent({
        data: {
          query: "app",
          requestId: "req-error",
          sessionId: "session-1",
        },
        messageId: "search-error",
        type: "searchContext",
      }),
    ).resolves.toBeUndefined();

    expect(postedFrames.at(-1)?.content).toEqual({
      matches: [],
      query: "app",
      requestId: "req-error",
      sessionId: "session-1",
      truncated: false,
      type: "contextSearchResult",
      workspaceAvailable: undefined,
    });

    expect(consoleErrorSpy).toHaveBeenCalled();
    consoleErrorSpy.mockRestore();
    findFilesSpy.mockRestore();
    provider.dispose();
  });
});

describe("mutation diff stat injection", () => {
  it("serializes mutation snapshot events so tool results cannot overtake tool starts", async () => {
    let emitEvent: ((event: Record<string, unknown>) => void) | undefined;
    let releaseStart:
      | ((value?: void | PromiseLike<void>) => void)
      | null = null;
    const rememberToolStart = vi.fn().mockImplementation(
      async () =>
        new Promise<void>((resolve) => {
          releaseStart = resolve;
        }),
    );
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/app.ts",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        rememberToolResult,
        rememberToolStart,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: (listener: (event: Record<string, unknown>) => void) => {
          emitEvent = listener;
          return { dispose() {} };
        },
      } as never,
      sessionRouter: {} as never,
    });

    emitEvent?.({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-edit-race",
      toolName: "edit",
      type: "tool_execution_start",
    });
    emitEvent?.({
      display: { file: "src/app.ts", kind: "file" },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-race",
      toolName: "edit",
      type: "tool_execution_end",
    });

    await Promise.resolve();
    expect(rememberToolStart).toHaveBeenCalledTimes(1);
    expect(rememberToolResult).not.toHaveBeenCalled();

    if (!releaseStart) {
      throw new Error("Expected queued tool-start release handle.");
    }
    (releaseStart as (value?: void | PromiseLike<void>) => void)(undefined);
    await vi.waitFor(() => {
      expect(rememberToolResult).toHaveBeenCalledWith(
        "tool-edit-race",
        "src/app.ts",
        undefined,
      );
    });

    provider.dispose();
  });

  it("keeps an errored edit tool settled as complete+error through turn_end and agent_idle", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {
        getState: vi.fn().mockResolvedValue({
          busy: false,
          sessionId: "s1",
        }),
        listCheckpoints: vi.fn().mockResolvedValue({
          checkpoints: [],
          sessionId: "s1",
        }),
      } as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_start",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      assistantMessageEvent: { delta: "updating file", kind: "content_delta" },
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-edit-err",
      toolName: "edit",
      type: "tool_execution_start",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: { file: "src/app.ts", kind: "file" },
      isError: true,
      result: "stale edit rejected",
      sessionId: "s1",
      toolCallId: "tool-edit-err",
      toolName: "edit",
      type: "tool_execution_end",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      toolCallIds: ["tool-edit-err"],
      toolResults: [{}],
      turnIndex: 0,
      type: "turn_end",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    const tool = provider
      .currentState()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tool-edit-err");
    expect(tool).toMatchObject({
      assistantMessageId: "assistant-1",
      isError: true,
      status: "complete",
      summary: "stale edit rejected",
      toolCallId: "tool-edit-err",
      toolName: "edit",
      type: "tool",
    });
    expect(provider.currentState().sessionViews.s1.busy).toBe(false);

    provider.dispose();
  });

  it("replaces a live raw error bubble with the hydrated transcript summary on agent_idle", async () => {
    const getMessages = vi.fn().mockResolvedValue({
      messages: [
        {
          detail: "LLM调用错误: API 错误 403: <!DOCTYPE html><html><title>403 Forbidden</title>",
          id: "history-error-1",
          summary: "API 错误 403 · aigateway.sunmi.com · Request-Id req-123",
          type: "error",
        },
      ],
      sessionId: "s1",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {
        getMessages,
        getState: vi.fn().mockResolvedValue({
          busy: true,
          sessionId: "s1",
        }),
      } as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_start",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      error: "LLM调用错误: API 错误 403: <!DOCTYPE html><html><title>403 Forbidden</title>",
      messages: [],
      sessionId: "s1",
      type: "agent_end",
    });

    expect(getMessages).not.toHaveBeenCalled();
    let errorBubble = provider
      .currentState()
      .sessionViews.s1.timeline.find((item) => item.type === "message" && item.kind === "error");
    expect(errorBubble).toMatchObject({
      text: "LLM调用错误: API 错误 403: <!DOCTYPE html><html><title>403 Forbidden</title>",
      type: "message",
    });
    expect(errorBubble && "detailText" in errorBubble ? errorBubble.detailText : undefined).toBeUndefined();

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    expect(getMessages).toHaveBeenCalledTimes(1);
    errorBubble = provider
      .currentState()
      .sessionViews.s1.timeline.find((item) => item.type === "message" && item.kind === "error");
    expect(errorBubble).toMatchObject({
      detailText: "LLM调用错误: API 错误 403: <!DOCTYPE html><html><title>403 Forbidden</title>",
      text: "API 错误 403 · aigateway.sunmi.com · Request-Id req-123",
      type: "message",
    });

    provider.dispose();
  });

  it("does not refresh history on a clean agent_end/agent_idle cycle", async () => {
    const getMessages = vi.fn().mockResolvedValue({
      messages: [],
      sessionId: "s1",
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {
        getMessages,
        getState: vi.fn().mockResolvedValue({
          busy: false,
          sessionId: "s1",
        }),
      } as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_start",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      error: null,
      messages: [],
      sessionId: "s1",
      type: "agent_end",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      sessionId: "s1",
      type: "agent_idle",
    });

    expect(getMessages).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("derives added/removed stats directly from file display metadata", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: {
        added: 2,
        diff: [
          { newLine: 1, oldLine: null, tag: "add", text: "export const x = 1;" },
          { newLine: 2, oldLine: null, tag: "add", text: "export const y = 2;" },
        ],
        file: "src/new.ts",
        kind: "file",
        removed: 0,
      },
      isError: false,
      result: "created file",
      sessionId: "s1",
      toolCallId: "tool-write-1",
      toolName: "write",
      type: "tool_execution_end",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: { added: 0, file: "src/steady.ts", kind: "file", removed: 0 },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    const tools = provider
      .currentState()
      .sessionViews.s1.timeline.filter((item) => item.type === "tool");
    expect(
      tools.find((tool) => tool.toolCallId === "tool-write-1"),
    ).toMatchObject({
      diff: [
        { newLine: 1, oldLine: null, tag: "add", text: "export const x = 1;" },
        { newLine: 2, oldLine: null, tag: "add", text: "export const y = 2;" },
      ],
      diffStat: {
        added: 2,
        removed: 0,
      },
      toolCallId: "tool-write-1",
    });
    expect(
      tools.find((tool) => tool.toolCallId === "tool-edit-1"),
    ).toMatchObject({
      diffStat: {
        added: 0,
        removed: 0,
      },
      toolCallId: "tool-edit-1",
    });

    provider.dispose();
  });

  it("keeps diff stats empty when file display omits counts", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: { file: "src/app.ts", kind: "file" },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    const tool = provider
      .currentState()
      .sessionViews.s1.timeline.find((item) => item.type === "tool" && item.toolCallId === "tool-edit-1");
    expect(tool).toMatchObject({
      toolCallId: "tool-edit-1",
      type: "tool",
    });
    expect(tool && "diffStat" in tool ? tool.diffStat : undefined).toBeUndefined();
    expect(tool && "diff" in tool ? tool.diff : undefined).toBeUndefined();

    provider.dispose();
  });

  it("routes openFile intents into ide.showFile with an optional line number", async () => {
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await provider.dispatchTestIntent({
      data: { line: 42, path: "src/app.ts" },
      messageId: "intent-open-file-1",
      type: "openFile",
    });

    expect(showFile).toHaveBeenCalledWith("src/app.ts", 42);

    provider.dispose();
  });

  it("routes chat links through the shared external-or-file classifier", async () => {
    const showFile = vi.fn().mockResolvedValue(undefined);
    const openExternal = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: { showFile } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      openExternal,
      sessionRouter: {} as never,
    });

    await provider.dispatchTestIntent({
      data: { href: "src/app.ts:59-103" },
      messageId: "intent-open-local-link",
      type: "openLink",
    });
    await provider.dispatchTestIntent({
      data: { href: "https://example.com/docs" },
      messageId: "intent-open-external-link",
      type: "openLink",
    });

    expect(showFile).toHaveBeenCalledWith("src/app.ts", 59);
    expect(openExternal).toHaveBeenCalledWith("https://example.com/docs");
    provider.dispose();
  });

  it("shows a toast instead of appending a transcript error when openFile fails", async () => {
    const showFile = vi.fn().mockRejectedValue(new Error("boom"));
    const toastMessages: string[] = [];
    __testing.setErrorMessageHandler((message) => {
      toastMessages.push(message);
      return undefined;
    });
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });
    const stateStore = (provider as unknown as { stateStore: { appendMessage: (...args: unknown[]) => void; setActiveSession(sessionId: string): void } }).stateStore;
    stateStore.setActiveSession("s1");
    const appendMessageSpy = vi.spyOn(stateStore, "appendMessage");

    await provider.dispatchTestIntent({
      data: { line: 42, path: "src/app.ts" },
      messageId: "intent-open-file-failure",
      type: "openFile",
    });

    expect(showFile).toHaveBeenCalledWith("src/app.ts", 42);
    expect(appendMessageSpy).not.toHaveBeenCalled();
    expect(toastMessages).toEqual([
      expect.stringContaining("Unable to open file src/app.ts"),
    ]);

    provider.dispose();
  });

  it("routes openDiff intents into ide.openReconstructedDiff", async () => {
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/app.ts",
    });
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        getPreparedChange: () => undefined,
        openReconstructedDiff,
        rememberToolResult,
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: {
        added: 1,
        diff: [
          { newLine: 1, oldLine: 1, tag: "ctx", text: "before" },
          { newLine: null, oldLine: 2, tag: "del", text: "old line" },
          { newLine: 2, oldLine: null, tag: "add", text: "new line" },
        ],
        file: "src/app.ts",
        kind: "file",
        removed: 1,
      },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-1",
      toolName: "edit",
      type: "tool_execution_end",
    });

    await provider.dispatchTestIntent({
      data: { toolCallId: "tool-edit-1" },
      messageId: "intent-open-diff-1",
      type: "openDiff",
    });

    expect(openReconstructedDiff).toHaveBeenCalledWith(
      "tool-edit-1",
      "src/app.ts",
      "before\nold line",
      "before\nnew line",
    );
    expect(showFile).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("reconstructs live diffs even when prepared changes already exist", async () => {
    const getPreparedChange = vi.fn().mockReturnValue({
      displayPath: "src/app.ts",
      existedBefore: true,
      hasStructuredDiff: true,
    });
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/app.ts",
    });
    const rememberToolStart = vi.fn().mockResolvedValue(undefined);
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        getPreparedChange,
        openReconstructedDiff,
        rememberToolResult,
        rememberToolStart,
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      args: { path: "src/app.ts" },
      sessionId: "s1",
      toolCallId: "tool-edit-live",
      toolName: "edit",
      type: "tool_execution_start",
    });
    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: {
        added: 1,
        diff: [
          { newLine: 1, oldLine: 1, tag: "ctx", text: "before" },
          { newLine: null, oldLine: 2, tag: "del", text: "old line" },
          { newLine: 2, oldLine: null, tag: "add", text: "new line" },
        ],
        file: "src/app.ts",
        kind: "file",
        removed: 1,
      },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-live",
      toolName: "edit",
      type: "tool_execution_end",
    });

    expect(rememberToolStart).toHaveBeenCalledWith("tool-edit-live", { path: "src/app.ts" });
    expect(rememberToolResult).toHaveBeenCalledWith("tool-edit-live", "src/app.ts", {
      after: "before\nnew line",
      before: "before\nold line",
    });

    await provider.dispatchTestIntent({
      data: { toolCallId: "tool-edit-live" },
      messageId: "intent-open-diff-live",
      type: "openDiff",
    });

    expect(getPreparedChange).not.toHaveBeenCalled();
    expect(openReconstructedDiff).toHaveBeenCalledWith(
      "tool-edit-live",
      "src/app.ts",
      "before\nold line",
      "before\nnew line",
    );
    expect(showFile).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("falls back to ide.showFile when openDiff has no structured diff", async () => {
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/huge.ts",
    });
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        getPreparedChange: () => undefined,
        openReconstructedDiff,
        rememberToolResult,
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: { added: 8, file: "src/huge.ts", kind: "file", removed: 2 },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-edit-2",
      toolName: "edit",
      type: "tool_execution_end",
    });

    await provider.dispatchTestIntent({
      data: { toolCallId: "tool-edit-2" },
      messageId: "intent-open-diff-2",
      type: "openDiff",
    });

    expect(showFile).toHaveBeenCalledWith("src/huge.ts");
    expect(openReconstructedDiff).not.toHaveBeenCalled();
    const session = provider.currentState().sessionViews.s1;
    expect(
      session.timeline.some(
        (item) =>
          item.type === "message" &&
          item.kind === "notice" &&
          item.text.includes("diff 过大或上下文不完整"),
      ),
    ).toBe(true);

    provider.dispose();
  });

  it("opens the current file rather than fabricating a reconstructed diff with gaps", async () => {
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {
        getPreparedChange: () => undefined,
        openReconstructedDiff,
        rememberToolResult: vi.fn().mockResolvedValue(undefined),
        showFile,
      } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: {} as never,
    });

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      display: {
        added: 1,
        diff: [
          { newLine: 1, oldLine: 1, tag: "ctx", text: "before gap" },
          { newLine: null, oldLine: null, tag: "gap", text: "90 unmodified lines" },
          { newLine: 92, oldLine: null, tag: "add", text: "changed" },
        ],
        file: "src/partial.ts",
        kind: "file",
        removed: 0,
      },
      isError: false,
      result: "updated file",
      sessionId: "s1",
      toolCallId: "tool-partial",
      toolName: "edit",
      type: "tool_execution_end",
    });

    await provider.dispatchTestIntent({
      data: { toolCallId: "tool-partial" },
      messageId: "intent-open-diff-partial",
      type: "openDiff",
    });

    expect(showFile).toHaveBeenCalledWith("src/partial.ts");
    expect(openReconstructedDiff).not.toHaveBeenCalled();
    provider.dispose();
  });
});

describe("checkpoint intent handling", () => {
  function createCheckpointProvider(sessionRouter: Partial<Record<string, unknown>> = {}) {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1", capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      sessionRouter: sessionRouter as never,
    });
  }

  it("dispatches restoreCheckpoint with revertFiles and refreshes state in order", async () => {
    const restoreCheckpoint = vi.fn().mockResolvedValue({
      checkpointId: "ck-1",
      revertFiles: false,
      sessionId: "s1",
    });
    const provider = createCheckpointProvider({ restoreCheckpoint });
    const ensureInitialized = vi
      .spyOn(provider as any, "ensureInitialized")
      .mockResolvedValue({ sessionId: "s1" } as never);
    const ensureWebviewSessionWithoutHistory = vi
      .spyOn(provider as any, "ensureWebviewSessionWithoutHistory")
      .mockResolvedValue("s1");
    const refreshSessionState = vi
      .spyOn(provider as any, "refreshSessionState")
      .mockResolvedValue(undefined);
    const refreshSessionHistory = vi
      .spyOn(provider as any, "refreshSessionHistory")
      .mockResolvedValue(undefined);
    const refreshCheckpoints = vi
      .spyOn(provider as any, "refreshCheckpoints")
      .mockResolvedValue(undefined);
    const refreshSessions = vi
      .spyOn(provider as any, "refreshSessions")
      .mockResolvedValue(undefined);
    const postState = vi.spyOn(provider as any, "postState").mockResolvedValue(undefined);

    await (provider as any).handleIntent({
      data: {
        checkpointId: "ck-1",
        revertFiles: false,
        sessionId: "s1",
      },
      messageId: "restore-1",
      type: "restoreCheckpoint",
    });

    expect(ensureInitialized).toHaveBeenCalled();
    expect(ensureWebviewSessionWithoutHistory).toHaveBeenCalledWith("s1");
    expect(restoreCheckpoint).toHaveBeenCalledWith("s1", "ck-1", false);
    expect(refreshSessionState).toHaveBeenCalledWith("s1", { trustBusy: true });
    expect(refreshSessionHistory).toHaveBeenCalledWith("s1");
    expect(refreshCheckpoints).toHaveBeenCalledWith("s1");
    expect(refreshSessions).toHaveBeenCalled();
    expect(postState).toHaveBeenCalled();
    expect(refreshSessionState.mock.invocationCallOrder[0]).toBeLessThan(
      refreshSessionHistory.mock.invocationCallOrder[0],
    );
    expect(refreshSessionHistory.mock.invocationCallOrder[0]).toBeLessThan(
      refreshCheckpoints.mock.invocationCallOrder[0],
    );
    expect(refreshCheckpoints.mock.invocationCallOrder[0]).toBeLessThan(
      refreshSessions.mock.invocationCallOrder[0],
    );

    provider.dispose();
  });

  it("stores checkpoint payloads returned by refreshCheckpoints", async () => {
    const provider = createCheckpointProvider({
      listCheckpoints: vi.fn().mockResolvedValue({
        checkpoints: [
          {
            changedFiles: ["src/app.ts"],
            createdAt: "2026-07-12T12:00:00Z",
            id: "ck-1",
            kind: "turn_end",
            messageAnchor: "assistant-1",
          },
        ],
        sessionId: "s1",
      }),
    });

    await (provider as any).refreshCheckpoints("s1");

    expect(provider.currentState().sessionViews.s1.checkpoints).toEqual([
      {
        changedFiles: ["src/app.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        label: null,
        messageAnchor: "assistant-1",
      },
    ]);

    provider.dispose();
  });

  it("refreshCheckpoints preserves the latest live turn while updating checkpoint payloads", async () => {
    const provider = createCheckpointProvider({
      listCheckpoints: vi.fn().mockResolvedValue({
        checkpoints: [
          {
            changedFiles: ["src/app.ts"],
            createdAt: "2026-07-12T12:00:00Z",
            id: "ck-1",
            kind: "turn_end",
            messageAnchor: "assistant-1",
          },
        ],
        sessionId: "s1",
      }),
    });
    const stateStore = (provider as unknown as { stateStore: Record<string, unknown> }).stateStore as {
      appendLocalUserMessage(
        sessionId: string,
        text: string,
        options: { messageId: string; submitKind: "prompt" | "steer" },
      ): void;
      applyEvent(frame: Record<string, unknown>): void;
      hydrateHistory(sessionId: string, history: Record<string, unknown>): void;
      markLocalUserMessageConfirmed(sessionId: string, messageId: string): void;
      setActiveSession(sessionId: string): void;
    };

    stateStore.setActiveSession("s1");
    stateStore.hydrateHistory("s1", {
      messages: [
        {
          id: "user-1",
          message: {
            content: "first prompt",
            role: "user",
          },
          type: "message",
        },
        {
          id: "assistant-1",
          message: {
            content: "first reply",
            role: "assistant",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    });
    stateStore.appendLocalUserMessage("s1", "latest prompt", {
      messageId: "user-2",
      submitKind: "prompt",
    });
    stateStore.markLocalUserMessageConfirmed("s1", "user-2");
    stateStore.applyEvent({
      assistantMessageEvent: { delta: "latest answer", kind: "content_delta" },
      assistantMessageId: "assistant-2",
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    stateStore.applyEvent({
      assistantMessageId: "assistant-2",
      message: {},
      sessionId: "s1",
      toolCallIds: [],
      toolResults: [],
      turnIndex: 1,
      type: "turn_end",
    });

    const before = provider.currentState().sessionViews.s1.timeline.map((item) => item.id);

    await (provider as any).refreshCheckpoints("s1");

    const session = provider.currentState().sessionViews.s1;
    expect(session.timeline.map((item) => item.id)).toEqual(before);
    expect(session.timeline.every((item) => item.type !== "checkpoint")).toBe(true);
    expect(session.checkpoints).toEqual([
      {
        changedFiles: ["src/app.ts"],
        createdAt: "2026-07-12T12:00:00Z",
        id: "ck-1",
        kind: "turn_end",
        label: null,
        messageAnchor: "assistant-1",
      },
    ]);

    provider.dispose();
  });
});

describe("plan build orchestration", () => {
  function createBuildProvider(
    messenger: Record<string, unknown>,
  ): TomcatWebviewViewProvider {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1", capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        ...messenger,
      } as never,
      sessionRouter: {} as never,
    });
  }

  function stubBuildInternals(provider: TomcatWebviewViewProvider): {
    postState: ReturnType<typeof vi.spyOn>;
    refreshModels: ReturnType<typeof vi.spyOn>;
    refreshSessionHistory: ReturnType<typeof vi.spyOn>;
    refreshSessionState: ReturnType<typeof vi.spyOn>;
  } {
    vi.spyOn(provider as any, "ensureInitialized").mockResolvedValue({ sessionId: "s1" } as never);
    vi.spyOn(provider as any, "ensureWebviewSessionWithoutHistory").mockResolvedValue("s1");
    const refreshModels = vi
      .spyOn(provider as any, "refreshModels")
      .mockResolvedValue(undefined);
    const refreshSessionState = vi
      .spyOn(provider as any, "refreshSessionState")
      .mockResolvedValue(undefined);
    const refreshSessionHistory = vi
      .spyOn(provider as any, "refreshSessionHistory")
      .mockResolvedValue(undefined);
    const postState = vi.spyOn(provider as any, "postState").mockResolvedValue(undefined);
    return { postState, refreshModels, refreshSessionHistory, refreshSessionState };
  }

  afterEach(() => {
    __testing.reset();
    vi.restoreAllMocks();
  });

  it("buildPlan applies the configured build model before entering build mode", async () => {
    __testing.setConfiguration("tomcat.plan.buildModel", "gpt-5.4");
    const sendSetModel = vi.fn().mockResolvedValue({ success: true });
    const sendSetPlanMode = vi.fn().mockResolvedValue({ success: true });
    const provider = createBuildProvider({ sendSetModel, sendSetPlanMode });
    const { refreshModels, refreshSessionHistory } = stubBuildInternals(provider);

    await provider.buildPlan("plan-1");

    expect(sendSetModel).toHaveBeenCalledWith("s1", "gpt-5.4");
    expect(sendSetPlanMode).toHaveBeenCalledWith({
      action: "build",
      planId: "plan-1",
      sessionId: "s1",
    });
    expect(sendSetModel.mock.invocationCallOrder[0]).toBeLessThan(
      sendSetPlanMode.mock.invocationCallOrder[0],
    );
    expect(refreshModels).toHaveBeenCalled();
    expect(refreshSessionHistory).toHaveBeenCalledWith("s1");

    provider.dispose();
  });

  it("abandons a build when its confirmed model is cleared while the dialog is open", async () => {
    __testing.setConfiguration("tomcat.plan.buildModel", "removed-model");
    const sendSetModel = vi.fn().mockResolvedValue({ success: true });
    const sendSetPlanMode = vi.fn().mockResolvedValue({ success: true });
    const provider = createBuildProvider({ sendSetModel, sendSetPlanMode });
    const { postState, refreshModels } = stubBuildInternals(provider);
    vi.spyOn(provider as any, "confirmBuildModel").mockImplementation(async () => {
      __testing.setConfiguration("tomcat.plan.buildModel", "");
      return true;
    });

    await provider.buildPlan("plan-1");

    expect(sendSetModel).not.toHaveBeenCalled();
    expect(sendSetPlanMode).not.toHaveBeenCalled();
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(postState).toHaveBeenCalledTimes(1);
    provider.dispose();
  });

  it("buildPlan skips the model switch when no build model is configured", async () => {
    const sendSetModel = vi.fn().mockResolvedValue({ success: true });
    const sendSetPlanMode = vi.fn().mockResolvedValue({ success: true });
    const provider = createBuildProvider({ sendSetModel, sendSetPlanMode });
    const { refreshModels } = stubBuildInternals(provider);

    await provider.buildPlan("plan-1");

    expect(sendSetModel).not.toHaveBeenCalled();
    expect(sendSetPlanMode).toHaveBeenCalledWith({
      action: "build",
      planId: "plan-1",
      sessionId: "s1",
    });
    expect(refreshModels).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("routes a card setPlanMode build intent through the same build path", async () => {
    __testing.setConfiguration("tomcat.plan.buildModel", "gpt-5.4");
    const sendSetModel = vi.fn().mockResolvedValue({ success: true });
    const sendSetPlanMode = vi.fn().mockResolvedValue({ success: true });
    const provider = createBuildProvider({ sendSetModel, sendSetPlanMode });
    stubBuildInternals(provider);

    await (provider as any).handleIntent({
      data: { action: "build", planId: "plan-1", sessionId: "s1" },
      messageId: "build-1",
      type: "setPlanMode",
    });

    expect(sendSetModel).toHaveBeenCalledWith("s1", "gpt-5.4");
    expect(sendSetPlanMode).toHaveBeenCalledWith({
      action: "build",
      planId: "plan-1",
      sessionId: "s1",
    });
    expect(sendSetModel.mock.invocationCallOrder[0]).toBeLessThan(
      sendSetPlanMode.mock.invocationCallOrder[0],
    );

    provider.dispose();
  });
});

describe("plan preview auto-open after review", () => {
  function makeProvider(
    openWith: ReturnType<typeof vi.fn>,
    showFile: ReturnType<typeof vi.fn>,
    refreshPlanPreview?: ReturnType<typeof vi.fn>,
  ): TomcatWebviewViewProvider {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: { openWith, showFile } as never,
      initialize: async () => ({ capabilities: [], slashCommands: [] } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      refreshPlanPreview,
      sessionRouter: {
        getState: vi.fn().mockResolvedValue({ busy: false, sessionId: "s1" }),
        listCheckpoints: vi.fn().mockResolvedValue({ checkpoints: [], sessionId: "s1" }),
      } as never,
    });
  }

  const emit = (provider: TomcatWebviewViewProvider, event: Record<string, unknown>) =>
    (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent(event);

  it("records plan.create and opens once when plan.review arrives", async () => {
    const openWith = vi.fn().mockResolvedValue(undefined);
    const provider = makeProvider(openWith, vi.fn().mockResolvedValue(undefined));
    const planPath = "/workspace/plans/new.plan.md";

    await emit(provider, { path: planPath, planId: "p1", sessionId: "s1", type: "plan.create" });
    expect(openWith).not.toHaveBeenCalled();

    await emit(provider, { planId: "p1", sessionId: "s1", summary: "looks good", type: "plan.review" });
    expect(openWith).toHaveBeenCalledTimes(1);
    expect(openWith).toHaveBeenCalledWith(planPath, "tomcat.planPreview");

    // Repeated create/review + later update for the same path must NOT steal focus again.
    await emit(provider, { path: planPath, planId: "p1", sessionId: "s1", type: "plan.create" });
    await emit(provider, { planId: "p1", sessionId: "s1", summary: "still good", type: "plan.review" });
    await emit(provider, {
      path: planPath,
      planId: "p1",
      sessionId: "s1",
      state: "executing",
      type: "plan.update",
    });
    expect(openWith).toHaveBeenCalledTimes(1);

    provider.dispose();
  });

  it("does not auto-open on plan.update, path-less create, or unknown plan.review", async () => {
    const openWith = vi.fn().mockResolvedValue(undefined);
    const provider = makeProvider(openWith, vi.fn().mockResolvedValue(undefined));

    await emit(provider, {
      path: "/workspace/plans/mid.plan.md",
      planId: "p1",
      sessionId: "s1",
      type: "plan.update",
    });
    await emit(provider, { planId: "p1", sessionId: "s1", type: "plan.create" });
    await emit(provider, { planId: "p1", sessionId: "s1", summary: "reviewed", type: "plan.review" });
    await emit(provider, { planId: "unknown", sessionId: "s1", summary: "reviewed", type: "plan.review" });
    expect(openWith).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("falls back to showFile when plan.review opening the custom editor throws", async () => {
    const openWith = vi.fn().mockRejectedValue(new Error("no custom editor"));
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = makeProvider(openWith, showFile);
    const planPath = "/workspace/plans/fallback.plan.md";

    await emit(provider, { path: planPath, planId: "p1", sessionId: "s1", type: "plan.create" });
    await emit(provider, { planId: "p1", sessionId: "s1", summary: "looks good", type: "plan.review" });
    expect(openWith).toHaveBeenCalledWith(planPath, "tomcat.planPreview");
    expect(showFile).toHaveBeenCalledWith(planPath);

    provider.dispose();
  });

  it("bridges plan.create/update/todos events into the preview refresher", async () => {
    const refreshPlanPreview = vi.fn().mockResolvedValue(undefined);
    const provider = makeProvider(
      vi.fn().mockResolvedValue(undefined),
      vi.fn().mockResolvedValue(undefined),
      refreshPlanPreview,
    );
    const planPath = "/workspace/plans/live.plan.md";

    await emit(provider, { path: planPath, planId: "p1", sessionId: "s1", type: "plan.create" });
    await emit(provider, {
      path: planPath,
      planId: "p1",
      sessionId: "s1",
      state: "executing",
      type: "plan.update",
    });
    await emit(provider, {
      planId: "p1",
      sessionId: "s1",
      todos: [{ content: "Live item", id: "t1", status: "pending" }],
      type: "plan.todos",
    });

    expect(refreshPlanPreview).toHaveBeenNthCalledWith(1, "p1", planPath, null);
    expect(refreshPlanPreview).toHaveBeenNthCalledWith(2, "p1", planPath, "executing");
    expect(refreshPlanPreview).toHaveBeenNthCalledWith(3, "p1", null, null);

    provider.dispose();
  });

  it("does not refresh the preview for unrelated serve events", async () => {
    const refreshPlanPreview = vi.fn().mockResolvedValue(undefined);
    const provider = makeProvider(
      vi.fn().mockResolvedValue(undefined),
      vi.fn().mockResolvedValue(undefined),
      refreshPlanPreview,
    );

    await emit(provider, { sessionId: "s1", type: "turn_end" });
    await emit(provider, {
      args: "{}",
      sessionId: "s1",
      toolCallId: "tc-1",
      toolName: "read",
      type: "tool_execution_start",
    });

    expect(refreshPlanPreview).not.toHaveBeenCalled();

    provider.dispose();
  });

  it("tolerates plan refresh events when no preview refresher is injected", async () => {
    const provider = makeProvider(vi.fn().mockResolvedValue(undefined), vi.fn().mockResolvedValue(undefined));

    await expect(
      emit(provider, {
        path: "/workspace/plans/nohook.plan.md",
        planId: "p1",
        sessionId: "s1",
        type: "plan.update",
      }),
    ).resolves.toBeUndefined();

    provider.dispose();
  });
});

describe("draft fork provider transaction", () => {
  function makeDraftForkProvider(failAt?: "create" | "retain" | "install" | "switch") {
    const order: string[] = [];
    const installedDrafts: unknown[] = [];
    const router = {
      createDetachedSession: vi.fn(async () => {
        order.push("create");
        if (failAt === "create") throw new Error("create failed");
        return "target-1";
      }),
      discardDetachedSession: vi.fn(async () => {
        order.push("discard-session");
        return true;
      }),
      retainAttachmentLeases: vi.fn(async (_sessionId: string, attachments: unknown[]) => {
        order.push("retain");
        if (failAt === "retain") throw new Error("retain failed");
        return attachments;
      }),
      switchSession: vi.fn(async () => {
        order.push("switch");
        if (failAt === "switch") throw new Error("switch failed");
        return "target-1";
      }),
    };
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ sessionId: "source-1", capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: router as never,
    });
    const draft = {
      attachments: [{
        blobSha: "a".repeat(64),
        bytes: 4,
        filename: "image.png",
        id: "attachment-1",
        kind: "image" as const,
        mimeType: "image/png",
        providerSha: "b".repeat(64),
      }],
      segments: [
        { text: "draft", type: "text" as const },
        {
          kind: "file" as const,
          label: "main.rs",
          path: "/workspace/main.rs",
          type: "reference" as const,
        },
      ],
      text: "draft",
    };
    const internals = provider as unknown as {
      adoptCommittedDraftFork(sessionId: string): Promise<void>;
      draftStore: {
        discardStrict(sessionId: string): Promise<void>;
        installIfEmpty(sessionId: string, draft: unknown): Promise<unknown>;
        peek(sessionId: string): typeof draft;
        replaceAndFlush(sessionId: string, draft: unknown): Promise<typeof draft>;
      };
      executeDraftFork(operation: Record<string, unknown>, capture: Record<string, unknown>): Promise<string>;
    };
    internals.draftStore = {
      discardStrict: async () => { order.push("discard-draft"); },
      installIfEmpty: async (_sessionId, value) => {
        order.push("install");
        if (failAt === "install") throw new Error("install failed");
        installedDrafts.push(value);
        return value;
      },
      peek: () => draft,
      replaceAndFlush: async (_sessionId, value) => {
        order.push("persist");
        return value as typeof draft;
      },
    };
    internals.adoptCommittedDraftFork = async () => { order.push("select"); };
    const execute = () => internals.executeDraftFork(
      {
        cwd: null,
        operationId: "operation-1",
        sourceSessionId: "source-1",
      },
      {
        cwd: null,
        operationId: "operation-1",
        segments: draft.segments,
        sourceSessionId: "source-1",
        text: draft.text,
      },
    );
    return { execute, installedDrafts, order, provider, router };
  }

  it("commits in the durable order and retains blob/provider hashes without bytes", async () => {
    const { execute, installedDrafts, order, provider, router } = makeDraftForkProvider();
    await expect(execute()).resolves.toBe("target-1");
    expect(order).toEqual(["persist", "create", "retain", "install", "switch", "select"]);
    expect(router.retainAttachmentLeases).toHaveBeenCalledWith("target-1", [{
      blobSha: "a".repeat(64),
      providerSha: "b".repeat(64),
    }]);
    expect(router.discardDetachedSession).not.toHaveBeenCalled();
    expect(installedDrafts).toEqual([
      expect.objectContaining({
        segments: [
          { text: "draft", type: "text" },
          {
            kind: "file",
            label: "main.rs",
            path: "/workspace/main.rs",
            type: "reference",
          },
        ],
      }),
    ]);
    provider.dispose();
  });

  it.each(["create", "retain", "install", "switch"] as const)(
    "compensates the detached target when %s fails before commit",
    async (phase) => {
      const { execute, order, provider, router } = makeDraftForkProvider(phase);
      await expect(execute()).rejects.toThrow(`${phase} failed`);
      if (phase === "create") {
        expect(router.discardDetachedSession).not.toHaveBeenCalled();
        expect(order).toEqual(["persist", "create"]);
      } else {
        expect(order.slice(-2)).toEqual(["discard-draft", "discard-session"]);
        expect(router.discardDetachedSession).toHaveBeenCalledWith("target-1");
      }
      expect(order).not.toContain("select");
      provider.dispose();
    },
  );
});

describe("serve connection readiness", () => {
  function createProvider(initialize: () => Promise<unknown>) {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: (async () => ({ capabilities: [], slashCommands: [], ...(await initialize() as object) })) as never,
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
  }

  it("does not mark serve ready merely because the webview DOM announces ready", async () => {
    const provider = createProvider(async () => {
      throw new Error("serve handshake failed");
    });

    await expect(
      provider.dispatchTestIntent({
        messageId: "webview-ready-before-handshake",
        type: "ready",
      }),
    ).rejects.toThrow("serve handshake failed");

    expect(provider.currentState()).toMatchObject({
      connectionStatus: "connecting",
      ready: false,
    });
    provider.dispose();
  });

  it("derives ready from the supervisor-owned serve connection state", async () => {
    const provider = createProvider(async () => ({}));

    await provider.setServeConnectionState("ready");
    expect(provider.currentState()).toMatchObject({
      connectionStatus: "ready",
      ready: true,
    });

    await provider.setServeConnectionState("reconnecting");
    expect(provider.currentState()).toMatchObject({
      connectionStatus: "reconnecting",
      ready: false,
    });
    provider.dispose();
  });

  it("waits for attachment-root reload before bootstrapping the replacement document", async () => {
    let resolveInitialize!: (value: unknown) => void;
    const initialize = vi.fn(
      () =>
        new Promise<unknown>((resolve) => {
          resolveInitialize = resolve;
        }),
    );
    const provider = createProvider(initialize);
    const bootstrap = vi.spyOn(provider as any, "bootstrap").mockResolvedValue(undefined);
    provider.resolveWebviewView({
      onDidChangeVisibility: () => new vscode.Disposable(() => undefined),
      show() {},
      visible: true,
      webview: {
        asWebviewUri: (uri: vscode.Uri) => uri,
        cspSource: "vscode-test-webview",
        html: "",
        onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
        options: {},
        postMessage: vi.fn().mockResolvedValue(true),
      },
    } as unknown as vscode.WebviewView);
    await provider.setServeConnectionState("ready");

    const invalidatedReady = provider.dispatchTestIntent({
      messageId: "webview-ready-before-attachment-root",
      type: "ready",
    });
    resolveInitialize({ attachmentRoot: "/storage/attachments", sessionId: "s1" });
    await invalidatedReady;
    expect(bootstrap).not.toHaveBeenCalled();

    await provider.dispatchTestIntent({
      messageId: "webview-ready-after-attachment-root",
      type: "ready",
    });
    expect(bootstrap).toHaveBeenCalledTimes(1);
    provider.dispose();
  });

  it("marks the connection degraded and reports a post-handshake bootstrap failure", async () => {
    const reportBootstrapFailure = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({
        capabilities: ["list_models"],
        sessionId: "s1",
      } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        sendListModels: async () => {
          throw new Error("list_models timed out");
        },
      } as never,
      reportBootstrapFailure,
      sessionRouter: {} as never,
    });
    await provider.setServeConnectionState("ready");

    await provider.dispatchTestIntent({
      messageId: "webview-ready-bootstrap-timeout",
      type: "ready",
    });

    expect(provider.currentState()).toMatchObject({
      connectionStatus: "degraded",
      ready: false,
    });
    // The supervisor can deliver its ready notification after `initialize()` resolves.
    // That notification describes the process, not bootstrap success, so it must not
    // overwrite the actionable degraded UI state.
    await provider.setServeConnectionState("ready");
    expect(provider.currentState()).toMatchObject({
      connectionStatus: "degraded",
      ready: false,
    });
    expect(reportBootstrapFailure).toHaveBeenCalledWith(
      expect.objectContaining({ message: "list_models timed out" }),
    );
    consoleError.mockRestore();
    provider.dispose();
  });

  it("publishes a degraded snapshot for a duplicate-timeline bootstrap failure", async () => {
    const postMessage = vi.fn().mockResolvedValue(true);
    const reportBootstrapFailure = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const provider = createProvider(async () => ({ sessionId: "s1" }));
    (provider as any).deps.reportBootstrapFailure = reportBootstrapFailure;
    provider.resolveWebviewView({
      onDidChangeVisibility: () => new vscode.Disposable(() => undefined),
      show() {},
      visible: true,
      webview: {
        asWebviewUri: (uri: vscode.Uri) => uri,
        cspSource: "vscode-test-webview",
        html: "",
        onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
        options: {},
        postMessage,
      },
    } as unknown as vscode.WebviewView);
    vi.spyOn(provider as any, "bootstrap").mockRejectedValue(
      new Error("duplicate timeline id in session s1: duplicate"),
    );

    await provider.setServeConnectionState("ready");
    await provider.dispatchTestIntent({
      messageId: "webview-ready-duplicate-timeline",
      type: "ready",
    });

    expect(provider.currentState()).toMatchObject({
      connectionStatus: "degraded",
      ready: false,
    });
    expect(reportBootstrapFailure).toHaveBeenCalledWith(
      expect.objectContaining({ message: "duplicate timeline id in session s1: duplicate" }),
    );
    expect(postMessage.mock.calls.some(([frame]) => frame.channel === "state")).toBe(true);
    consoleError.mockRestore();
    provider.dispose();
  });

  it("reports a bootstrap failure even when publishing degraded state fails", async () => {
    const reportBootstrapFailure = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1", capabilities: [], slashCommands: [] } as never),
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      reportBootstrapFailure,
      sessionRouter: {} as never,
    });
    vi.spyOn(provider as any, "bootstrap").mockRejectedValue(new Error("history failed"));
    vi.spyOn(provider as any, "postState").mockRejectedValueOnce(new Error("state publish failed"));

    await provider.setServeConnectionState("ready");
    await provider.dispatchTestIntent({
      messageId: "webview-ready-post-state-failure",
      type: "ready",
    });

    expect(reportBootstrapFailure).toHaveBeenCalledWith(
      expect.objectContaining({ message: "history failed" }),
    );
    expect(consoleError).toHaveBeenCalledWith(
      "[Tomcat webview] failed to publish degraded bootstrap state",
      expect.objectContaining({ message: "state publish failed" }),
    );
    consoleError.mockRestore();
    provider.dispose();
  });

  it("keeps a ready connection when list_models is not an advertised capability", async () => {
    const sendListModels = vi.fn();
    const reportBootstrapFailure = vi.fn();
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({
        capabilities: ["get_state"],
        sessionId: "s1",
      } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        sendListModels,
      } as never,
      reportBootstrapFailure,
      sessionRouter: {} as never,
    });
    vi.spyOn(provider as any, "bootstrap").mockImplementation(async () => {
      await (provider as any).refreshModels({ strict: true });
    });
    (provider as any).isReady = true;
    await provider.setServeConnectionState("ready");

    await provider.retryBootstrap();

    expect(sendListModels).not.toHaveBeenCalled();
    expect(reportBootstrapFailure).not.toHaveBeenCalled();
    expect(provider.currentState()).toMatchObject({
      availableModels: [],
      connectionStatus: "ready",
      ready: true,
    });
    provider.dispose();
  });

  it("returns from degraded to ready when a bootstrap retry succeeds", async () => {
    const sendListModels = vi
      .fn()
      .mockRejectedValueOnce(new Error("first list_models failure"))
      .mockResolvedValueOnce({
        payload: { models: [] },
        success: true,
      });
    const reportBootstrapFailure = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({
        capabilities: ["list_models"],
        sessionId: "s1",
      } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        sendListModels,
      } as never,
      reportBootstrapFailure,
      sessionRouter: {} as never,
    });
    vi.spyOn(provider as any, "bootstrap").mockImplementation(async () => {
      await (provider as any).refreshModels({ strict: true });
    });
    (provider as any).isReady = true;
    await provider.setServeConnectionState("ready");

    await provider.retryBootstrap();
    expect(provider.currentState().connectionStatus).toBe("degraded");

    await provider.retryBootstrap();
    expect(provider.currentState()).toMatchObject({
      availableModels: [],
      connectionStatus: "ready",
      ready: true,
    });
    expect(sendListModels).toHaveBeenCalledTimes(2);
    expect(reportBootstrapFailure).toHaveBeenCalledTimes(1);
    consoleError.mockRestore();
    provider.dispose();
  });
});
