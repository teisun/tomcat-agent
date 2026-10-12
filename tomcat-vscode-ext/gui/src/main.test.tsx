import { afterEach, describe, expect, it, vi } from "vitest";
import { act, screen, waitFor } from "@testing-library/react";

import type { VsCodeApiLike } from "./types";

describe("chat webview entry point", () => {
  afterEach(() => {
    document.body.replaceChildren();
    Reflect.deleteProperty(window, "acquireVsCodeApi");
    vi.resetModules();
  });

  it("webview_render_smoke_mounts_full_tree", async () => {
    const postMessage = vi.fn();
    const vscodeApi: VsCodeApiLike = {
      getState: vi.fn(),
      postMessage,
      setState: vi.fn(),
    };
    document.body.innerHTML = '<div id="root"></div>';
    Object.defineProperty(window, "acquireVsCodeApi", {
      configurable: true,
      value: vi.fn(() => vscodeApi),
    });

    await act(async () => {
      await import("./main");
    });

    await waitFor(() => {
      expect(document.querySelector(".tc-shell")).not.toBeNull();
      expect(document.querySelector('[data-testid="stream-container"]')).not.toBeNull();
      expect(document.querySelector('[data-testid="composer-bar"]')).not.toBeNull();
    });
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        messageId: expect.stringMatching(/^ready-/u),
        type: "ready",
      }),
    );
  });

  it("test-only crash fixture renders the boundary fallback and reports the host", async () => {
    const postMessage = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const vscodeApi: VsCodeApiLike = {
      getState: vi.fn(),
      postMessage,
      setState: vi.fn(),
    };
    document.body.innerHTML = '<div id="root"></div>';
    Object.defineProperty(window, "acquireVsCodeApi", {
      configurable: true,
      value: vi.fn(() => vscodeApi),
    });

    try {
      await act(async () => {
        await import("./main");
      });

      await act(async () => {
        window.dispatchEvent(
          new MessageEvent("message", {
            data: {
              channel: "event",
              content: {
                enabled: true,
                type: "__test.webview_error_boundary_crash",
              },
              messageId: "e2e-error-boundary-crash",
            },
          }),
        );
      });

      await waitFor(() => {
        expect(screen.getByTestId("webview-error-fallback").textContent).toContain(
          "E2E fixture intentionally crashed the Tomcat webview",
        );
      });
      expect(postMessage).toHaveBeenCalledWith(
        expect.objectContaining({
          data: expect.objectContaining({
            message: "E2E fixture intentionally crashed the Tomcat webview",
          }),
          type: "webviewError",
        }),
      );
    } finally {
      consoleError.mockRestore();
    }
  });
});
