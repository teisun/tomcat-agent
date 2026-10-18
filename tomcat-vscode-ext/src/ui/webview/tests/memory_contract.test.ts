/**
 * Memory contracts for image attachments.
 *
 * These are regression tests for a specific, measured failure: eleven 4.5MB photos in a
 * composer cost roughly half a gigabyte, because the same bytes existed as base64 in the
 * host, again as base64 in every state snapshot, and again as an undownsampled bitmap in
 * Chromium. Each assertion here pins one of the invariants that removed a copy. They are
 * written as structural assertions rather than measurements so they fail at review time
 * rather than in a memory profile six months from now.
 */
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterAll, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import {
  attachmentResourceRoots,
  resolveAttachmentUris,
} from "../../../shared/attachmentUris";
import { WebviewStateStore } from "../state";
import { TomcatWebviewViewProvider } from "../provider";
import type { WebviewPendingAttachment } from "../protocol";

const ATTACHMENT_ROOT = "/home/u/.tomcat/agents/main/sessions/attachments";

function sha(seed: number): string {
  return seed.toString(16).padStart(2, "0").repeat(32);
}

const webviewStub = {
  asWebviewUri: (uri: vscode.Uri) => vscode.Uri.parse(`https://webview.test${uri.path}`),
  cspSource: "https://webview.test",
} as unknown as vscode.Webview;

function pendingAttachment(index: number): WebviewPendingAttachment {
  const uris = resolveAttachmentUris(webviewStub, ATTACHMENT_ROOT, {
    blobSha: sha(index),
    hasThumb: true,
  });
  return {
    blobSha: sha(index),
    bytes: 4_500_000,
    filename: `photo-${index}.jpg`,
    fullUri: uris?.fullUri ?? null,
    hasThumb: true,
    id: `att-${index}`,
    kind: "image",
    label: `photo-${index}.jpg`,
    mimeType: "image/jpeg",
    thumbUri: uris?.thumbUri ?? null,
  };
}

function snapshotFor(attachmentCount: number): string {
  const store = new WebviewStateStore();
  store.setPendingAttachments(
    "s1",
    Array.from({ length: attachmentCount }, (_, index) => pendingAttachment(index)),
  );
  return JSON.stringify(store.snapshot());
}

function snapshotWithMediaRoots(attachmentCount: number): string {
  return JSON.stringify({
    ...JSON.parse(snapshotFor(attachmentCount)),
    mediaRoots: [
      {
        fsPath: "/workspace",
        webviewBase: "https://webview.test/workspace",
      },
      {
        fsPath: os.tmpdir(),
        webviewBase: "https://webview.test/tmp",
      },
    ],
  });
}

describe("snapshot memory contract", () => {
  it("costs a bounded, tiny amount per attachment regardless of image size", () => {
    const withNone = snapshotFor(0).length;
    const withOne = snapshotFor(1).length;
    const withEleven = snapshotFor(11).length;

    // Growth must be linear in the *number* of attachments and independent of their
    // size: what a snapshot carries is a hash and two URLs, and those are the same
    // length for a 12KB icon and a 4.5MB photo.
    const perAttachment = withOne - withNone;
    expect(perAttachment).toBeLessThan(600);
    expect(withEleven - withNone).toBeLessThan(perAttachment * 12);

    // The number that used to matter: eleven photos at 4.5MB each is ~66MB of base64.
    // The whole snapshot now has to fit in a few kilobytes.
    expect(withEleven).toBeLessThan(8 * 1024);
  });

  it("puts no image bytes anywhere in a steady-state snapshot", () => {
    const snapshot = snapshotFor(11);

    expect(snapshot).not.toContain("dataBase64");
    expect(snapshot).not.toContain("data:image");
    expect(snapshot).not.toContain("base64");
  });

  it("serializes mediaRoots once per snapshot, not once per attachment", () => {
    const rootsOnlyOverhead = snapshotWithMediaRoots(0).length - snapshotFor(0).length;
    const rootsWithElevenImages = snapshotWithMediaRoots(11).length - snapshotFor(11).length;

    expect(rootsWithElevenImages).toBe(rootsOnlyOverhead);
  });

  it("describes attachments purely by reference", () => {
    const store = new WebviewStateStore();
    store.setPendingAttachments("s1", [pendingAttachment(0)]);

    const attachment = store.snapshot().sessionViews.s1?.pendingAttachments[0];

    expect(attachment).toEqual({
      blobSha: sha(0),
      bytes: 4_500_000,
      filename: "photo-0.jpg",
      fullUri: `https://webview.test${ATTACHMENT_ROOT}/blobs/${sha(0)}`,
      hasThumb: true,
      id: "att-0",
      kind: "image",
      label: "photo-0.jpg",
      mimeType: "image/jpeg",
      thumbUri: `https://webview.test${ATTACHMENT_ROOT}/thumbs/${sha(0)}`,
    });
  });
});

describe("attachment URI mapping", () => {
  it("addresses a thumbnail by the hash of its source image", () => {
    const uris = resolveAttachmentUris(webviewStub, ATTACHMENT_ROOT, {
      blobSha: sha(7),
      hasThumb: true,
    });

    // One hash, two paths. A thumbnail with its own hash would need a lookup table, and
    // a lookup table is a thing that can get out of sync.
    expect(uris?.fullUri).toContain(`/blobs/${sha(7)}`);
    expect(uris?.thumbUri).toContain(`/thumbs/${sha(7)}`);
  });

  // The single most expensive line this codebase has had: pointing a thumbnail-less
  // attachment at the original reads as a harmless "heavier but correct" fallback, and it
  // is how eleven 4000x3000 photos came to cost 480MB of decoded bitmaps in a strip of
  // 48px squares. Anything that arrives without a thumbnail must render as a placeholder
  // until one is generated.
  it("never substitutes the full image when a thumbnail is missing", () => {
    const uris = resolveAttachmentUris(webviewStub, ATTACHMENT_ROOT, {
      blobSha: sha(7),
      hasThumb: false,
    });

    expect(uris?.fullUri).toContain(`/blobs/${sha(7)}`);
    expect(uris?.thumbUri).toBeNull();
  });

  it("refuses to build a URL from anything that is not a sha256", () => {
    for (const blobSha of ["", "../../etc/passwd", "z".repeat(64), sha(1).slice(0, 63)]) {
      expect(resolveAttachmentUris(webviewStub, ATTACHMENT_ROOT, { blobSha })).toBeNull();
    }
  });

  it("grants exactly the two directories that hold readable bytes", () => {
    const roots = attachmentResourceRoots(ATTACHMENT_ROOT).map((uri) => uri.path);

    expect(roots).toEqual([`${ATTACHMENT_ROOT}/blobs`, `${ATTACHMENT_ROOT}/thumbs`]);
    // `pending/` holds lease markers, not bytes. Nothing in the UI should be able to
    // read it, so it is not granted.
    expect(roots.some((root) => root.endsWith("/pending"))).toBe(false);
  });

  it("returns nothing at all when the attachment root is unknown", () => {
    expect(attachmentResourceRoots(null)).toEqual([]);
    expect(
      resolveAttachmentUris(webviewStub, null, { blobSha: sha(1) }),
    ).toBeNull();
  });

  /**
   * Remote-SSH, Dev Containers and WSL all run the extension on the far side, where the
   * attachment root is a path on *that* filesystem and the webview runs locally. VS Code
   * bridges the two by rewriting the resource URL, which only works if we hand it a `Uri`
   * and let it do the rewriting — a hand-built `file://` string, or anything that reads
   * the bytes locally, breaks there and nowhere else.
   */
  it("keeps remote paths intact so VS Code can proxy them", () => {
    // What `asWebviewUri` does on a remote window: the whole remote path is preserved
    // under a `/vscode-remote/<authority>` prefix.
    const remoteWebview = {
      asWebviewUri: (uri: vscode.Uri) =>
        vscode.Uri.parse(
          `https://x.vscode-cdn.net/vscode-remote/ssh-remote%2Bbuildbox${uri.path}`,
        ),
      cspSource: "https://x.vscode-cdn.net",
    } as unknown as vscode.Webview;
    const remoteRoot = "/home/deploy/.tomcat/agents/main/sessions/attachments";

    const uris = resolveAttachmentUris(remoteWebview, remoteRoot, {
      blobSha: sha(3),
      hasThumb: true,
    });

    expect(uris?.fullUri).toBe(
      `https://x.vscode-cdn.net/vscode-remote/ssh-remote%2Bbuildbox${remoteRoot}/blobs/${sha(3)}`,
    );
    expect(uris?.thumbUri).toBe(
      `https://x.vscode-cdn.net/vscode-remote/ssh-remote%2Bbuildbox${remoteRoot}/thumbs/${sha(3)}`,
    );
    // The resource roots handed to the webview are extension-host paths, which on a
    // remote window means paths on the remote machine. Rewriting them locally would be
    // the bug.
    expect(attachmentResourceRoots(remoteRoot).map((uri) => uri.path)).toEqual([
      `${remoteRoot}/blobs`,
      `${remoteRoot}/thumbs`,
    ]);
  });
});

describe("history images rebuilt from a transcript", () => {
  /** A transcript entry as `get_messages` returns it in reference mode: a hash, no bytes. */
  function historyWithImage(blobSha: string) {
    return {
      messages: [
        {
          id: "user-1",
          message: {
            content: [
              { text: "look at this", type: "text" },
              {
                blobSha,
                filename: "diagram.png",
                hasThumb: true,
                mime_type: "image/png",
                type: "input_image",
              },
            ],
            role: "user",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    };
  }

  function historyImageIn(store: WebviewStateStore) {
    const message = store
      .snapshot()
      .sessionViews.s1?.timeline.find((item) => item.type === "message");
    return message?.type === "message" ? message.attachments?.[0] : undefined;
  }

  function historyWithPdf(blobSha: string) {
    return {
      messages: [
        {
          id: "user-1",
          message: {
            content: [
              { text: "read this", type: "text" },
              {
                blobSha,
                bytes: 4096,
                filename: "brief.pdf",
                mime_type: "application/pdf",
                type: "input_file",
              },
            ],
            role: "user",
          },
          type: "message",
        },
      ],
      sessionId: "s1",
    };
  }

  // A transcript names images by hash and nothing else, so every session switch and every
  // reopened window rebuilds them with no address. Left unresolved they render as
  // placeholders that never turn into images, which reads as a permanent loading state.
  it("gets URLs from the injected resolver", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.setAttachmentUriResolver((attachment) => ({
      fullUri: `https://webview.test/blobs/${attachment.blobSha}`,
      thumbUri: `https://webview.test/thumbs/${attachment.blobSha}`,
    }));
    store.hydrateHistory("s1", historyWithImage(sha(5)));

    expect(historyImageIn(store)).toMatchObject({
      fullUri: `https://webview.test/blobs/${sha(5)}`,
      thumbUri: `https://webview.test/thumbs/${sha(5)}`,
    });
  });

  it("resolves images that were rebuilt before the resolver existed", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    // The attachment root arrives from the handshake, which can land after the first
    // history load. Images loaded in between must not stay address-less.
    store.hydrateHistory("s1", historyWithImage(sha(6)));
    expect(historyImageIn(store)?.fullUri).toBeUndefined();

    store.setAttachmentUriResolver(() => ({
      fullUri: "https://webview.test/blobs/late",
      thumbUri: null,
    }));

    expect(historyImageIn(store)?.fullUri).toBe("https://webview.test/blobs/late");
  });

  it("marks an image whose bytes are gone as unavailable", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.setAttachmentUriResolver(() => ({
      fullUri: null,
      thumbUri: null,
      unavailable: true,
    }));
    store.hydrateHistory("s1", historyWithImage(sha(7)));

    // Without this the bubble shows an empty square forever: the hash is still in the
    // transcript, but the blob behind it was collected.
    expect(historyImageIn(store)).toMatchObject({ unavailable: true });
  });

  it("keeps history PDFs reference-only, with no base64 sneaking back in", () => {
    const store = new WebviewStateStore();
    store.setActiveSession("s1");
    store.setAttachmentUriResolver((attachment) => ({
      fullUri: `https://webview.test/blobs/${attachment.blobSha}`,
      thumbUri: null,
    }));
    store.hydrateHistory("s1", historyWithPdf(sha(8)));

    const message = store
      .snapshot()
      .sessionViews.s1?.timeline.find((item) => item.type === "message");
    const attachment = message?.type === "message" ? message.attachments?.[0] : undefined;

    expect(attachment).toMatchObject({
      blobSha: sha(8),
      fullUri: `https://webview.test/blobs/${sha(8)}`,
      kind: "file",
      mimeType: "application/pdf",
    });
    expect(JSON.stringify(message)).not.toContain("dataBase64");
  });
});

describe("shared tool thumbnail cache", () => {
  it.each([false, true])("adopts a shared thumbnail across sessions (stored before second=%s)", async storedFirst => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-shared-thumb-"));
    const blobSha = sha(9);
    await fs.mkdir(path.join(root, "blobs"));
    await fs.mkdir(path.join(root, "thumbs"));
    await fs.writeFile(path.join(root, "blobs", blobSha), "fixture-original");
    const request = vi.fn(async () => {
      await fs.writeFile(path.join(root, "thumbs", blobSha), "fixture-thumbnail");
      return { success: true };
    });
    const provider = new TomcatWebviewViewProvider({
      draftStorage: { globalStorageUri: vscode.Uri.file(path.join(root, "drafts")), storageUri: undefined },
      extensionUri: vscode.Uri.file("/ext"), getDefaultCwd: () => "/workspace", ide: {} as never,
      initialize: async () => ({}) as never,
      messenger: { onEvent: () => ({ dispose() {} }), request } as never, sessionRouter: {} as never,
    });
    const host = provider as unknown as {
      attachmentRoot: string; view: unknown; stateStore: WebviewStateStore;
      historyAttachmentResolver(): import("../state").AttachmentUriResolver;
      storeGeneratedThumbnail(data: { blobSha: string; sessionId: string; thumbBase64: string }): Promise<void>;
    };
    host.attachmentRoot = root;
    host.view = { webview: webviewStub };
    host.stateStore.setAttachmentUriResolver(host.historyAttachmentResolver());
    const addTool = (sessionId: string, callId: string) => {
      host.stateStore.setActiveSession(sessionId);
      host.stateStore.applyEvent({ type: "tool_execution_end", sessionId, toolCallId: callId, toolName: "read",
        result: "[Image attached]", isError: false,
        media: [{ type: "input_image_ref", blobSha, mimeType: "image/png", bytes: 16 }] });
    };
    try {
      addTool("first", "call-first");
      if (storedFirst) await host.storeGeneratedThumbnail({ blobSha, sessionId: "first", thumbBase64: "AA==" });
      addTool("second", "call-second");
      if (!storedFirst) await host.storeGeneratedThumbnail({ blobSha, sessionId: "first", thumbBase64: "AA==" });
      for (const id of ["first", "second"]) {
        const tool = provider.currentState().sessionViews[id].timeline.find(item => item.type === "tool");
        expect(tool).toMatchObject({ attachments: [{ blobSha, hasThumb: true,
          thumbUri: `https://webview.test${vscode.Uri.joinPath(vscode.Uri.file(root), "thumbs", blobSha).path}` }] });
      }
      expect(request).toHaveBeenCalledOnce();
      expect(provider.currentState().activeSessionId).toBe("second");
      expect(JSON.stringify(provider.currentState())).not.toContain("fixture-original");
    } finally { provider.dispose(); await fs.rm(root, { recursive: true, force: true }); }
  });
});

describe("chat webview security headers", () => {
  const tempDirs: string[] = [];

  afterAll(async () => {
    await Promise.all(
      tempDirs.map((dir) => fs.rm(dir, { force: true, recursive: true })),
    );
  });

  // renderHtml reads the real built assets, so the CSP can only be asserted against a
  // root that actually has them.
  async function extensionRootWithAssets(): Promise<vscode.Uri> {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-csp-"));
    tempDirs.push(dir);
    await fs.mkdir(path.join(dir, "gui", "dist"), { recursive: true });
    await fs.writeFile(
      path.join(dir, "gui", "dist", "index.html"),
      '<!doctype html><html><head><script type="module" src="./index.js"></script></head><body><div id="root"></div></body></html>',
      "utf8",
    );
    await fs.writeFile(
      path.join(dir, "gui", "dist", "index.js"),
      "console.log('index');",
      "utf8",
    );
    return vscode.Uri.file(dir);
  }

  function providerWithRoot(
    attachmentRoot: string | null,
    extensionUri = vscode.Uri.file("/ext"),
  ): TomcatWebviewViewProvider {
    const provider = new TomcatWebviewViewProvider({
      extensionUri,
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({}) as never,
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
    (provider as unknown as { attachmentRoot: string | null }).attachmentRoot =
      attachmentRoot;
    return provider;
  }

  it("allows images only from the webview origin and blob:, never data:", async () => {
    const html = (
      providerWithRoot(ATTACHMENT_ROOT, await extensionRootWithAssets()) as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(webviewStub);

    expect(html).toContain("img-src https://webview.test blob:;");
    // `data:` in the policy is what makes a base64 fallback possible. Without it, any
    // regression that reintroduces inline bytes fails visibly instead of quietly
    // working and costing half a gigabyte.
    expect(html).not.toContain("data:");
    expect(html).toContain("default-src 'none'");
  });

  it("grants the attachment directories to the webview, and only once known", () => {
    const workspace = vscode.workspace as typeof vscode.workspace & {
      workspaceFolders: vscode.WorkspaceFolder[];
    };
    workspace.workspaceFolders = [
      {
        index: 0,
        name: "workspace",
        uri: vscode.Uri.file("/workspace"),
      },
    ];
    const withoutRoot = (
      providerWithRoot(null) as unknown as { resourceRoots(): vscode.Uri[] }
    ).resourceRoots();
    const withRoot = (
      providerWithRoot(ATTACHMENT_ROOT) as unknown as {
        resourceRoots(): vscode.Uri[];
      }
    ).resourceRoots();

    expect(withoutRoot.map((uri) => uri.path)).toEqual([
      "/ext/gui/dist",
      "/ext/media",
      "/workspace",
      vscode.Uri.file(os.tmpdir()).path,
    ]);
    expect(withRoot.map((uri) => uri.path)).toEqual([
      "/ext/gui/dist",
      "/ext/media",
      "/workspace",
      vscode.Uri.file(os.tmpdir()).path,
      `${ATTACHMENT_ROOT}/blobs`,
      `${ATTACHMENT_ROOT}/thumbs`,
    ]);
  });
});

describe("typing does not push state", () => {
  const typingDirs: string[] = [];

  afterAll(async () => {
    await Promise.all(
      typingDirs.map((dir) => fs.rm(dir, { force: true, recursive: true })),
    );
  });

  it("saves a draft keystroke without broadcasting a snapshot", async () => {
    const provider = new TomcatWebviewViewProvider({
      extensionUri: vscode.Uri.file("/ext"),
      getDefaultCwd: () => "/workspace",
      ide: {} as never,
      initialize: async () => ({}) as never,
      messenger: { onEvent: () => ({ dispose() {} }) } as never,
      sessionRouter: {} as never,
    });
    const postState = vi
      .spyOn(provider as never, "postState")
      .mockResolvedValue(undefined as never);
    vi.spyOn(provider as never, "applyDraftToState").mockResolvedValue(
      undefined as never,
    );

    await (
      provider as unknown as {
        handleWebviewMessage(message: unknown): Promise<void>;
      }
    ).handleWebviewMessage({
      data: { segments: [], sessionId: "s1", text: "hello" },
      messageId: "m1",
      type: "syncComposerDraft",
    });

    // The draft came *from* the webview. Echoing the whole snapshot back on every 250ms
    // of typing is pure cost, and with attachments in the snapshot it used to be a large
    // one.
    expect(postState).not.toHaveBeenCalled();
  });

  /**
   * The same claim as above, measured end to end instead of by spy.
   *
   * The spy proves one method is not called; this counts the bytes that actually reach
   * the webview while someone types with eleven images attached. It is the number the
   * plan asks for at delivery, and the only version of it that cannot be satisfied by a
   * refactor that renames `postState` and keeps sending the snapshot.
   */
  it("sends zero bytes to the webview while typing, with or without attachments", async () => {
    async function bytesSentWhileTyping(attachmentCount: number): Promise<number> {
      const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-typing-"));
      typingDirs.push(dir);
      const posted: unknown[] = [];
      const provider = new TomcatWebviewViewProvider({
        draftStorage: { globalStorageUri: vscode.Uri.file(dir), storageUri: undefined },
        extensionUri: vscode.Uri.file("/ext"),
        getDefaultCwd: () => "/workspace",
        ide: {} as never,
        initialize: async () => ({}) as never,
        messenger: { onEvent: () => ({ dispose() {} }) } as never,
        sessionRouter: {} as never,
      });
      const internals = provider as unknown as {
        attachmentRoot: null | string;
        draftStore: {
          update(
            sessionId: string,
            mutate: (current: { attachments: unknown[] }) => unknown,
          ): unknown;
        };
        handleWebviewMessage(message: unknown): Promise<void>;
        isReady: boolean;
        stateStore: { setReady(ready: boolean): void };
        view: unknown;
      };
      internals.view = {
        visible: true,
        webview: {
          asWebviewUri: (uri: vscode.Uri) =>
            vscode.Uri.parse(`https://webview.test${uri.path}`),
          cspSource: "https://webview.test",
          postMessage: async (frame: unknown) => {
            posted.push(frame);
            return true;
          },
        },
      };
      internals.isReady = true;
      internals.stateStore.setReady(true);
      // Attachments live in the draft, so this is also what makes them part of every
      // snapshot the host could push.
      internals.attachmentRoot = null;
      internals.draftStore.update("s1", (current) => ({
        ...current,
        attachments: Array.from({ length: attachmentCount }, (_, index) => ({
          blobSha: sha(index),
          bytes: 4_500_000,
          filename: `photo-${index}.jpg`,
          hasThumb: true,
          id: `att-${index}`,
          kind: "image" as const,
          mimeType: "image/jpeg",
        })),
      }));

      // A short burst of realistic keystroke batches, at the rate the composer debounces.
      const typed = "the quick brown fox jumps over the lazy dog";
      for (let length = 1; length <= typed.length; length += 2) {
        await internals.handleWebviewMessage({
          data: { segments: [], sessionId: "s1", text: typed.slice(0, length) },
          messageId: `m${length}`,
          type: "syncComposerDraft",
        });
      }

      return posted.reduce<number>(
        (total, frame) => total + JSON.stringify(frame).length,
        0,
      );
    }

    expect(await bytesSentWhileTyping(0)).toBe(0);
    // Independent of N because it is independent of everything: typing produces no
    // outbound traffic at all.
    expect(await bytesSentWhileTyping(11)).toBe(0);
  });
});
