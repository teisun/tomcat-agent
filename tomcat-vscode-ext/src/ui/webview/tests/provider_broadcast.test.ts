import * as os from "node:os";

import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import { TomcatWebviewViewProvider } from "../provider";
import type { HostToWebviewFrame } from "../protocol";

const workspace = vscode.workspace as typeof vscode.workspace & {
  onDidChangeWorkspaceFolders?: (
    listener: (event: vscode.WorkspaceFoldersChangeEvent) => void,
  ) => vscode.Disposable;
  workspaceFolders?: Array<{ uri: vscode.Uri }>;
};
const originalWorkspaceFolders = workspace.workspaceFolders;
const originalWorkspaceFolderListener = workspace.onDidChangeWorkspaceFolders;

function makeWorkspaceFolder(fsPath: string, index = 0): vscode.WorkspaceFolder {
  return {
    index,
    name: fsPath.split("/").filter(Boolean).at(-1) ?? fsPath,
    uri: vscode.Uri.file(fsPath),
  };
}

function createProvider() {
  const postedFrames: HostToWebviewFrame[] = [];
  const provider = new TomcatWebviewViewProvider({
    extensionUri: vscode.Uri.file("/workspace/extension"),
    getDefaultCwd: () => "/workspace",
    ide: {} as never,
    initialize: async () => ({ sessionId: "s1" } as never),
    messenger: {
      onEvent: () => ({ dispose() {} }),
    } as never,
    sessionRouter: {
      getState: vi.fn().mockResolvedValue({
        busy: false,
        model: "gpt-5.4",
        sessionId: "s1",
        thinkingLevel: "high",
      }),
      listCheckpoints: vi.fn().mockResolvedValue({
        checkpoints: [],
        sessionId: "s1",
      }),
    } as never,
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

  const internals = provider as unknown as {
    isReady: boolean;
    stateStore: { setReady(ready: boolean): void };
  };
  internals.isReady = true;
  internals.stateStore.setReady(true);

  return { postedFrames, provider };
}

afterEach(() => {
  vi.useRealTimers();
  delete process.env.TOMCAT_DISABLE_SESSION_PATCHES;
  workspace.onDidChangeWorkspaceFolders = originalWorkspaceFolderListener;
  workspace.workspaceFolders = originalWorkspaceFolders;
});

describe("provider broadcast frames", () => {

  it("projects the newest in-memory draft when publishing a full state frame", async () => {
    const { postedFrames, provider } = createProvider();
    const internals = provider as unknown as {
      draftStore: { update(sessionId: string, mutate: (draft: { text: string }) => { text: string }): void };
      postStateFrame(): Promise<void>;
      stateStore: { setComposerDraft(sessionId: string, draft: { segments: []; text: string }): void };
    };
    internals.stateStore.setComposerDraft("s1", { segments: [], text: "old snapshot text" });
    internals.draftStore.update("s1", (draft) => ({ ...draft, text: "newest typed text" }));

    await internals.postStateFrame();

    const frame = postedFrames.at(-1);
    expect(frame).toMatchObject({
      channel: "state",
      content: { sessionViews: { s1: { composerDraft: { text: "newest typed text" } } } },
    });
    provider.dispose();
  });
  it("emits sessionPatch frames for streaming message deltas", async () => {
    const { postedFrames, provider } = createProvider();

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      assistantMessageEvent: { delta: "hello", kind: "content_delta" },
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      type: "message_update",
    });

    await (
      provider as unknown as {
        stateBroadcaster: { forceFlush(): Promise<void> };
      }
    ).stateBroadcaster.forceFlush();

    const patchFrames = postedFrames.filter(
      (frame): frame is Extract<HostToWebviewFrame, { channel: "sessionPatch" }> =>
        frame.channel === "sessionPatch",
    );
    expect(patchFrames).toHaveLength(1);
    expect(patchFrames[0]).toMatchObject({
      channel: "sessionPatch",
      content: {
        ops: [
          {
            item: {
              id: "assistant-1",
              kind: "assistant",
              text: "hello",
              type: "message",
            },
            type: "upsert",
          },
        ],
        seq: 1,
        sessionId: "s1",
      },
    });

    provider.dispose();
  });

  it("forces a sessionView flush at turn_end", async () => {
    const { postedFrames, provider } = createProvider();

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      assistantMessageEvent: { delta: "hello", kind: "content_delta" },
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
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      toolCallIds: [],
      toolResults: [],
      turnIndex: 0,
      type: "turn_end",
    });

    const sessionViewFrames = postedFrames.filter(
      (frame): frame is Extract<HostToWebviewFrame, { channel: "sessionView" }> =>
        frame.channel === "sessionView",
    );
    expect(sessionViewFrames).toHaveLength(1);
    expect(sessionViewFrames[0].content).toMatchObject({
      sessionId: "s1",
      view: {
        checkpoints: [],
        sessionId: "s1",
      },
    });

    provider.dispose();
  });

  it("falls back to sessionView frames when session patches are disabled", async () => {
    process.env.TOMCAT_DISABLE_SESSION_PATCHES = "1";
    const { postedFrames, provider } = createProvider();

    await (
      provider as unknown as {
        handleServeEvent(event: Record<string, unknown>): Promise<void>;
      }
    ).handleServeEvent({
      assistantMessageEvent: { delta: "hello", kind: "content_delta" },
      assistantMessageId: "assistant-1",
      message: {},
      sessionId: "s1",
      type: "message_update",
    });
    await (
      provider as unknown as {
        stateBroadcaster: { forceFlush(): Promise<void> };
      }
    ).stateBroadcaster.forceFlush();

    expect(postedFrames.some((frame) => frame.channel === "sessionPatch")).toBe(false);
    expect(postedFrames.filter((frame) => frame.channel === "sessionView")).toHaveLength(1);

    provider.dispose();
  });

  it("emits a background-session patch without re-sending the foreground full state", async () => {
    const { postedFrames, provider } = createProvider();
    const internals = provider as unknown as {
      currentState(): {
        activeSessionId: string | null;
        sessionViews: Record<string, { timeline: unknown[] }>;
      };
      handleServeEvent(event: Record<string, unknown>): Promise<void>;
      stateBroadcaster: { forceFlush(): Promise<void> };
      stateStore: {
        setActiveSession(sessionId: string): void;
        syncSessionList(payload: {
          activeSessionId: string;
          scope: "live";
          sessions: Array<{
            busy: boolean;
            isCurrent: boolean;
            sessionId: string;
            title: string | null;
            updatedAt: number | null;
          }>;
        }): void;
      };
    };
    internals.stateStore.syncSessionList({
      activeSessionId: "s1",
      scope: "live",
      sessions: [
        { busy: false, isCurrent: true, sessionId: "s1", title: "Foreground", updatedAt: 1 },
        { busy: false, isCurrent: false, sessionId: "s2", title: "Background", updatedAt: 2 },
      ],
    });
    internals.stateStore.setActiveSession("s1");

    await internals.handleServeEvent({
      assistantMessageEvent: { delta: "hello", kind: "content_delta" },
      assistantMessageId: "assistant-2",
      message: {},
      sessionId: "s2",
      type: "message_update",
    });
    await internals.stateBroadcaster.forceFlush();

    const patchFrames = postedFrames.filter(
      (frame): frame is Extract<HostToWebviewFrame, { channel: "sessionPatch" }> =>
        frame.channel === "sessionPatch",
    );
    expect(patchFrames).toHaveLength(1);
    expect(patchFrames[0].content.sessionId).toBe("s2");
    expect(postedFrames.some((frame) => frame.channel === "state")).toBe(false);
    expect(internals.currentState().activeSessionId).toBe("s1");
    expect(internals.currentState().sessionViews.s1?.timeline).toHaveLength(0);

    const patchBytes = JSON.stringify(patchFrames[0]).length;
    const fullStateBytes = JSON.stringify({
      channel: "state",
      content: internals.currentState(),
      messageId: "state-now",
    }).length;
    expect(patchBytes).toBeLessThan(fullStateBytes);

    provider.dispose();
  });

  it("includes workspace and temp media roots in decorated state snapshots", () => {
    workspace.workspaceFolders = [makeWorkspaceFolder("/workspace")];

    const { provider } = createProvider();
    const state = (provider as unknown as {
      currentState(): {
        mediaRoots?: Array<{ fsPath: string; webviewBase: string }>;
      };
    }).currentState();

    expect(state.mediaRoots).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ fsPath: "/workspace" }),
        expect.objectContaining({ fsPath: os.tmpdir() }),
      ]),
    );

    provider.dispose();
  });

  it("reloads resource roots when workspace folders change", () => {
    let listener: ((event: vscode.WorkspaceFoldersChangeEvent) => void) | undefined;
    workspace.workspaceFolders = [makeWorkspaceFolder("/workspace")];
    workspace.onDidChangeWorkspaceFolders = (nextListener) => {
      listener = nextListener;
      return new vscode.Disposable(() => {
        listener = undefined;
      });
    };

    const { provider } = createProvider();
    const internals = provider as unknown as {
      isReady: boolean;
      stateStore: { setReady(ready: boolean): void };
      view: {
        webview: {
          options: { localResourceRoots?: vscode.Uri[] };
        };
      };
    };
    internals.isReady = true;
    internals.stateStore.setReady(true);

    const nextWorkspaceFolders = [
      makeWorkspaceFolder("/workspace", 0),
      makeWorkspaceFolder("/workspace-two", 1),
    ];
    workspace.workspaceFolders = nextWorkspaceFolders;
    listener?.({ added: [nextWorkspaceFolders[1]], removed: [] });

    expect(internals.isReady).toBe(false);
    expect(internals.view.webview.options.localResourceRoots?.map((uri) => uri.path)).toEqual(
      expect.arrayContaining(["/workspace", "/workspace-two"]),
    );

    provider.dispose();
  });
});
