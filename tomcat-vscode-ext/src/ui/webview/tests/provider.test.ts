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
});

describe("model catalog parsing", () => {
  it("retains per-model capability metadata for the webview", () => {
    expect(
      parseModelCatalog({
        models: [
          {
            capabilities: {
              reasoning: true,
            },
            id: "deepseek-v4-flash",
            keyPresent: true,
          },
          {
            capabilities: ["vision", "files"],
            id: "gpt-5.4",
            keyPresent: true,
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
    });
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
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        releaseAll() {},
      } as never,
      sessionRouter: {} as never,
    });

    const html = (
      provider as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain('rel="stylesheet"');
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
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        releaseAll() {},
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
});

function buildSearchProvider(): {
  postedFrames: HostToWebviewFrame[];
  provider: TomcatWebviewViewProvider;
} {
  const postedFrames: HostToWebviewFrame[] = [];
  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/workspace/extension"),
    getDefaultCwd: () => "/workspace",
    getUiMode: () => "webview",
    ide: {} as never,
    initialize: async () => ({} as never),
    messenger: {
      onEvent: () => ({ dispose() {} }),
    } as never,
    ownership: {
      releaseAll() {},
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
  it("keeps an errored edit tool settled as complete+error through turn_end and agent_idle", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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

  it("derives added/removed stats directly from file display metadata", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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

  it("routes openDiff intents into ide.openReconstructedDiff", async () => {
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/app.ts",
    });
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "webview",
      ide: {
        getPreparedChange: () => undefined,
        openReconstructedDiff,
        rememberToolResult,
        showFile,
      } as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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

  it("prefers ide.openPreparedDiff when live tool snapshots are available", async () => {
    const getPreparedChange = vi.fn().mockReturnValue({
      displayPath: "src/app.ts",
    });
    const openPreparedDiff = vi.fn().mockResolvedValue(undefined);
    const openReconstructedDiff = vi.fn().mockResolvedValue(undefined);
    const rememberToolResult = vi.fn().mockResolvedValue({
      displayPath: "src/app.ts",
    });
    const rememberToolStart = vi.fn().mockResolvedValue(undefined);
    const showFile = vi.fn().mockResolvedValue(undefined);
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "webview",
      ide: {
        getPreparedChange,
        openPreparedDiff,
        openReconstructedDiff,
        rememberToolResult,
        rememberToolStart,
        showFile,
      } as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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
    expect(rememberToolResult).toHaveBeenCalledWith("tool-edit-live", "src/app.ts");

    await provider.dispatchTestIntent({
      data: { toolCallId: "tool-edit-live" },
      messageId: "intent-open-diff-live",
      type: "openDiff",
    });

    expect(getPreparedChange).toHaveBeenCalledWith("tool-edit-live");
    expect(openPreparedDiff).toHaveBeenCalledWith("tool-edit-live");
    expect(openReconstructedDiff).not.toHaveBeenCalled();
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
      getUiMode: () => "webview",
      ide: {
        getPreparedChange: () => undefined,
        openReconstructedDiff,
        rememberToolResult,
        showFile,
      } as never,
      initialize: async () => ({} as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        ownerOf() {
          return null;
        },
        releaseAll() {},
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

    provider.dispose();
  });
});

describe("checkpoint intent handling", () => {
  function createCheckpointProvider(sessionRouter: Partial<Record<string, unknown>> = {}) {
    return new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/workspace/extension"),
      getDefaultCwd: () => "/workspace",
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1" } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
      } as never,
      ownership: {
        claim() {
          return { ok: true, record: { owner: "webview" } };
        },
        ownerOf() {
          return { owner: "webview" };
        },
        releaseAll() {},
        snapshot() {
          return {};
        },
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
      getUiMode: () => "webview",
      ide: {} as never,
      initialize: async () => ({ sessionId: "s1" } as never),
      messenger: {
        onEvent: () => ({ dispose() {} }),
        ...messenger,
      } as never,
      ownership: {
        ownerOf() {
          return { owner: "webview" };
        },
        releaseAll() {},
      } as never,
      sessionRouter: {} as never,
    });
  }

  function stubBuildInternals(provider: TomcatWebviewViewProvider): {
    postState: ReturnType<typeof vi.spyOn>;
    refreshModels: ReturnType<typeof vi.spyOn>;
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
    const postState = vi.spyOn(provider as any, "postState").mockResolvedValue(undefined);
    return { postState, refreshModels, refreshSessionState };
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
    const { refreshModels } = stubBuildInternals(provider);

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
