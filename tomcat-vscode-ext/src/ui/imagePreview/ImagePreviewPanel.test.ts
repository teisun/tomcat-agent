import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { translate } from "../../shared/i18n";

const mocks = vi.hoisted(() => {
  const posted: unknown[] = [];
  const panel = {
    dispose: vi.fn(),
    onDidDispose: vi.fn(),
    reveal: vi.fn(),
    webview: {
      asWebviewUri: vi.fn((uri: { fsPath: string }) => ({
        toString: () => `vscode-resource:${uri.fsPath}`,
      })),
      cspSource: "vscode-webview:",
      html: "",
      onDidReceiveMessage: vi.fn(),
      postMessage: vi.fn(async (message: unknown) => {
        posted.push(message);
        return true;
      }),
    },
  };
  return {
    disposeHandler: undefined as (() => void) | undefined,
    messageHandler: undefined as ((message: any) => Promise<void>) | undefined,
    panel,
    posted,
    copy: vi.fn(),
    saveUri: undefined as { fsPath: string } | undefined,
  };
});

vi.mock("vscode", () => ({
  Uri: {
    file: (fsPath: string) => ({ fsPath }),
    joinPath: (base: { fsPath: string }, ...parts: string[]) => ({
      fsPath: [base.fsPath, ...parts].join("/"),
    }),
  },
  ViewColumn: { Active: 1 },
  window: {
    createWebviewPanel: vi.fn(() => {
      mocks.panel.onDidDispose.mockImplementation((handler: () => void) => {
        mocks.disposeHandler = handler;
      });
      mocks.panel.webview.onDidReceiveMessage.mockImplementation(
        (handler: (message: any) => Promise<void>) => {
          mocks.messageHandler = handler;
        },
      );
      mocks.panel.dispose.mockImplementation(() => mocks.disposeHandler?.());
      return mocks.panel;
    }),
    showSaveDialog: vi.fn(async () => mocks.saveUri),
  },
  workspace: {
    fs: { copy: mocks.copy },
  },
}));

vi.mock("../guiAssets", () => ({
  resolveWebviewEntryAssets: () => ({
    scripts: ["/ext/gui/dist/image-preview.js"],
    stylesheets: ["/ext/gui/dist/styles.css"],
  }),
}));

import { ImagePreviewPanel } from "./ImagePreviewPanel";

// Pictures are addressed by hash now, so the fixtures carry webview URLs instead of
// inline bytes. `save` copies straight from the blob store, which is why the test also
// pins the attachment root.
const ATTACHMENT_ROOT = "/tomcat/sessions/attachments";
const SHA_ONE = "1".repeat(64);
const SHA_TWO = "2".repeat(64);

const pictures = [
  {
    filename: "one.png",
    fullUri: `https://webview.local/blobs/${SHA_ONE}`,
    id: "one",
    mimeType: "image/png",
    thumbUri: `https://webview.local/thumbs/${SHA_ONE}`,
  },
  {
    filename: "two.png",
    fullUri: `https://webview.local/blobs/${SHA_TWO}`,
    id: "two",
    mimeType: "image/png",
    thumbUri: `https://webview.local/thumbs/${SHA_TWO}`,
  },
];

beforeEach(() => {
  mocks.posted.length = 0;
  mocks.saveUri = undefined;
  mocks.copy.mockClear();
  mocks.messageHandler = undefined;
  mocks.disposeHandler = undefined;
  vi.clearAllMocks();
});

afterEach(() => {
  ImagePreviewPanel.getCurrent()?.close();
});

describe("ImagePreviewPanel host", () => {
  it("creates one reusable panel with strict image CSP and pushes ready state", async () => {
    const first = ImagePreviewPanel.getInstance({ fsPath: "/ext" } as any, ATTACHMENT_ROOT);
    first.reveal([{ label: "Pending", pictures }], "two");
    const second = ImagePreviewPanel.getInstance({ fsPath: "/ext" } as any, ATTACHMENT_ROOT);
    expect(second).toBe(first);
    // data: must be gone from the policy: with it in place a base64 fallback could be
    // reintroduced later and would silently work, taking the memory blow-up with it.
    expect(mocks.panel.webview.html).not.toContain("data:");
    expect(mocks.panel.webview.html).toContain("img-src vscode-webview: blob:");
    expect(mocks.panel.webview.html).toMatch(/script-src 'nonce-[A-Za-z0-9]+' 'strict-dynamic'/);
    expect(mocks.panel.webview.html).not.toContain("script-src vscode-webview:");
    expect(mocks.panel.webview.html).toMatch(/<script nonce="[A-Za-z0-9]+" type="module"/);
    await mocks.messageHandler?.({ data: {}, type: "preview.ready" });
    expect(mocks.posted).toContainEqual(
      expect.objectContaining({
        data: expect.objectContaining({ activeId: "two", position: 2, total: 2 }),
        type: "preview.state",
      }),
    );
  });

  it("keeps the nearest active image on collection changes and closes when empty", async () => {
    const panel = ImagePreviewPanel.getInstance({ fsPath: "/ext" } as any, ATTACHMENT_ROOT);
    panel.reveal([{ label: "Pending", pictures }], "two");
    panel.updateSections([{ label: "Pending", pictures: [pictures[0]] }]);
    expect(mocks.posted.at(-1)).toEqual(
      expect.objectContaining({
        data: expect.objectContaining({ activeId: "one", total: 1 }),
      }),
    );
    panel.updateSections([]);
    expect(mocks.panel.dispose).toHaveBeenCalled();
  });

  it.each(["not-found", "missing-bytes", "copy-failed", "dialog-failed"])("reports %s as an error, never as a saved path", async (failure) => {
    const panel = ImagePreviewPanel.getInstance({ fsPath: "/ext" } as any, ATTACHMENT_ROOT);
    const picture = failure === "missing-bytes" ? { ...pictures[0], fullUri: "https://webview.local/missing" } : pictures[0];
    panel.reveal([{ label: "Pending", pictures: [picture] }], "one");
    mocks.saveUri = { fsPath: "/tmp/one.png" };
    if (failure === "copy-failed") mocks.copy.mockRejectedValueOnce(new Error("copy failed"));
    if (failure === "dialog-failed") vi.mocked(vscode.window.showSaveDialog).mockRejectedValueOnce(new Error("dialog failed"));
    const error = failure === "not-found" ? translate("en", "preview.notFound")
      : failure === "missing-bytes" ? translate("en", "preview.bytesMissing")
      : failure === "dialog-failed" ? "Error: dialog failed" : "Error: copy failed";
    await mocks.messageHandler?.({
      type: "preview.save", data: { attachmentId: failure === "not-found" ? "missing" : "one" },
    });
    await vi.waitFor(() => expect(mocks.posted.at(-1)).toEqual({
      type: "preview.saveResult", data: { cancelled: false, error, savedPath: null, success: false },
    }));
    expect(mocks.copy).toHaveBeenCalledTimes(failure === "copy-failed" ? 1 : 0);
  });

  it("reports Save As cancellation without an error and copies the blob on success", async () => {
    const panel = ImagePreviewPanel.getInstance({ fsPath: "/ext" } as any, ATTACHMENT_ROOT);
    panel.reveal([{ label: "Pending", pictures }], "one");
    await mocks.messageHandler?.({
      data: { attachmentId: "one" },
      type: "preview.save",
    });
    expect(mocks.posted.at(-1)).toEqual({
      data: {
        cancelled: true,
        error: null,
        savedPath: null,
        success: false,
      },
      type: "preview.saveResult",
    });

    mocks.saveUri = { fsPath: "/tmp/one.png" };
    await mocks.messageHandler?.({
      data: { attachmentId: "one" },
      type: "preview.save",
    });
    // Saving copies file-to-file out of the blob store; the bytes never enter the
    // extension host's heap.
    expect(mocks.copy).toHaveBeenCalledWith(
      { fsPath: `${ATTACHMENT_ROOT}/blobs/${SHA_ONE}` },
      mocks.saveUri,
      { overwrite: true },
    );
    await vi.waitFor(() =>
      expect(mocks.posted.at(-1)).toEqual(
        expect.objectContaining({
          data: expect.objectContaining({
            cancelled: false,
            savedPath: "/tmp/one.png",
            success: true,
          }),
        }),
      ),
    );  });
});
