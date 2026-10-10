import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PreviewSection } from "../../../src/shared/imagePreviewProtocol";
import { COPY_FLASH_MS } from "../components/copyFeedback";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";
import {
  PreviewPanel,
  clipboardBlobForPicture,
  collectPictures,
  setPreviewVsCodeApiForTests,
} from "./PreviewPanel";

const postMessage = vi.fn();

function sections(count = 11): PreviewSection[] {
  return [
    {
      label: "Pending",
      pictures: Array.from({ length: count }, (_, index) => ({
        filename: `image-${index + 1}.png`,
        fullUri: `https://webview.local/blobs/full-${index + 1}`,
        id: `image-${index + 1}`,
        mimeType: "image/png",
        thumbUri: `https://webview.local/thumbs/thumb-${index + 1}`,
      })),
    },
  ];
}

function pushState(position = 2, count = 11): void {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: {
          data: {
            activeId: `image-${position}`,
            displayLabel: `Attached image ${position}`,
            position,
            sections: sections(count),
            total: count,
          },
          type: "preview.state",
        },
      }),
    );
  });
}

beforeEach(() => {
  postMessage.mockReset();
  setPreviewVsCodeApiForTests({
    getState: () => undefined,
    postMessage,
    setState: vi.fn(),
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("PreviewPanel", () => {
  it("switches controls in place without resetting image, zoom or filmstrip", () => {
    const view = render(<LocaleProvider locale="en"><PreviewPanel /></LocaleProvider>);
    pushState();
    fireEvent.click(screen.getByRole("button", { name: translate("en", "preview.zoomIn") }));
    const next = screen.getByRole("button", { name: translate("en", "preview.next") });
    const copy = screen.getByRole("button", { name: translate("en", "preview.copy") });
    const image = screen.getByTestId("preview-stage-image");
    const stage = screen.getByTestId("preview-stage");
    const active = screen.getByRole("button", { name: /image-2.png — 2 of 11/ });
    const readyCount = postMessage.mock.calls.filter(([m]) => m.type === "preview.ready").length;
    view.rerender(<LocaleProvider locale="zh-CN"><PreviewPanel /></LocaleProvider>);
    expect(screen.getByTestId("preview-stage-image")).toBe(image);
    expect(stage.getAttribute("data-zoom")).toBe("1.5");
    expect(document.querySelector('.ip-thumb--active[aria-current="true"]')).toBe(active);
    expect(screen.getByTestId("preview-copy")).toBe(copy);
    fireEvent.click(next);
    expect(postMessage).toHaveBeenCalledWith({ type: "preview.select", data: { attachmentId: "image-3" } });
    expect(postMessage.mock.calls.filter(([m]) => m.type === "preview.ready")).toHaveLength(readyCount);
  });

  it("keeps the header label single-line with a full tooltip while controls retain their width", () => {
    render(<PreviewPanel />);
    pushState();
    const title = screen.getByTitle("Attached image 2");
    expect(title.style.whiteSpace).toBe("nowrap");
    expect(title.style.textOverflow).toBe("ellipsis");
    expect(screen.getByText("2 / 11").style.whiteSpace).toBe("nowrap");
    expect(screen.getByRole("button", { name: "Close (Escape)" }).style.flex).toBe("0 0 auto");
  });

  it("flattens sections in stable display order", () => {
    expect(collectPictures(sections(3)).map((picture) => picture.id)).toEqual([
      "image-1",
      "image-2",
      "image-3",
    ]);
  });

  it("renders item 2 of 11, highlights its thumbnail and exposes bounded navigation", async () => {
    render(<PreviewPanel />);
    pushState();
    expect(await screen.findByText("2 / 11")).toBeTruthy();
    expect(screen.getByAltText("image-2.png")).toBeTruthy();
    expect(
      screen
        .getByRole("button", { name: /image-2\.png — 2 of 11/ })
        .getAttribute("aria-current"),
    ).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(postMessage).toHaveBeenCalledWith({
      data: { attachmentId: "image-3" },
      type: "preview.select",
    });
    pushState(11);
    expect(await screen.findByText("11 / 11")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("supports keyboard navigation, fit/zoom and Escape close", async () => {
    render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(postMessage).toHaveBeenCalledWith({
      data: { attachmentId: "image-1" },
      type: "preview.select",
    });
    fireEvent.keyDown(window, { key: "+" });
    expect(screen.getByTestId("preview-stage").getAttribute("data-zoom")).toBe("1.5");
    fireEvent.keyDown(window, { key: "0" });
    expect(screen.getByTestId("preview-stage").getAttribute("data-zoom")).toBe("fit");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(postMessage).toHaveBeenCalledWith({ data: {}, type: "preview.close" });
  });

  it("renders codicon toolbar and filmstrip icons, including the stateful fit toggle", async () => {
    render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");

    expect(
      screen.getByRole("button", { name: "Zoom out" }).querySelector(".codicon-zoom-out"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Actual size" }).querySelector(".codicon-screen-normal"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Zoom in" }).querySelector(".codicon-zoom-in"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Copy image" }).querySelector(".codicon-copy"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /Save as/ }).querySelector(".codicon-download"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Previous" }).querySelector(".codicon-chevron-left"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Next" }).querySelector(".codicon-chevron-right"),
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Actual size" }));
    const fitButton = screen.getByRole("button", { name: "Fit to window" });
    expect(fitButton.querySelector(".codicon-screen-full")).toBeTruthy();
  });

  it("posts Save As and announces success, cancellation and failure accessibly", async () => {
    render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");
    fireEvent.click(screen.getByRole("button", { name: /Save as/ }));
    expect(postMessage).toHaveBeenCalledWith({
      data: { attachmentId: "image-2" },
      type: "preview.save",
    });
    act(() => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: { data: { cancelled: true, success: false }, type: "preview.saveResult" },
        }),
      );
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("Save cancelled"),
    );
    act(() => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: { data: { success: true }, type: "preview.saveResult" },
        }),
      );
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("Image saved"),
    );
  });

  it("copies binary PNG data with ClipboardItem", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    class ClipboardItemMock {
      static supports(type: string): boolean {
        return type === "image/png";
      }
      constructor(readonly items: Record<string, Blob>) {}
    }
    vi.stubGlobal("ClipboardItem", ClipboardItemMock);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        arrayBuffer: async () => new Uint8Array([1]).buffer,
        ok: true,
      }),
    );
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { write },
    });
    render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");
    const copyButton = screen.getByRole("button", { name: "Copy image" });
    // Own the feedback clock: machine load must not consume the flash before assertions.
    vi.useFakeTimers();
    await act(async () => { fireEvent.click(copyButton); });
    expect(write).toHaveBeenCalledTimes(1);
    expect(copyButton.classList.contains("is-copied")).toBe(true);
    expect(copyButton.querySelector(".codicon-check")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Image copied");
    act(() => { vi.advanceTimersByTime(COPY_FLASH_MS); });
    expect(copyButton.classList.contains("is-copied")).toBe(false);
    expect(copyButton.querySelector(".codicon-copy")).toBeTruthy();
  });

  it("keeps the copy icon when clipboard write fails", async () => {
    const write = vi.fn().mockRejectedValue(new Error("nope"));
    class ClipboardItemMock {
      static supports(type: string): boolean {
        return type === "image/png";
      }
      constructor(readonly items: Record<string, Blob>) {}
    }
    vi.stubGlobal("ClipboardItem", ClipboardItemMock);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        arrayBuffer: async () => new Uint8Array([1]).buffer,
        ok: true,
      }),
    );
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { write },
    });
    render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");
    const copyButton = screen.getByRole("button", { name: "Copy image" });
    fireEvent.click(copyButton);
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("Copy failed"),
    );
    expect(copyButton.classList.contains("is-copied")).toBe(false);
    expect(copyButton.querySelector(".codicon-copy")).toBeTruthy();
    expect(copyButton.querySelector(".codicon-check")).toBeNull();
  });

  it("clears the copy reset timer on unmount", async () => {
    const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
    const write = vi.fn().mockResolvedValue(undefined);
    class ClipboardItemMock {
      static supports(type: string): boolean {
        return type === "image/png";
      }
      constructor(readonly items: Record<string, Blob>) {}
    }
    vi.stubGlobal("ClipboardItem", ClipboardItemMock);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        arrayBuffer: async () => new Uint8Array([1]).buffer,
        ok: true,
      }),
    );
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { write },
    });
    const { unmount } = render(<PreviewPanel />);
    pushState();
    await screen.findByText("2 / 11");
    fireEvent.click(screen.getByRole("button", { name: "Copy image" }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    unmount();
    expect(clearTimeoutSpy).toHaveBeenCalled();
  });

  // The bytes come back typed from the attachment's own mime type, not from the resource
  // protocol, which reports everything hash-named as unknown. Without that the clipboard
  // would refuse the image and an SVG would not render at all.
  it("restates the picture's media type when reading bytes for the clipboard", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        arrayBuffer: async () => new Uint8Array([1, 2]).buffer,
        ok: true,
      }),
    );
    vi.stubGlobal("ClipboardItem", class { static supports() { return true; } });
    const blob = await clipboardBlobForPicture(sections(1)[0].pictures[0]);
    expect(blob.type).toBe("image/png");
    expect(blob.size).toBe(2);
  });

  // The memory contract for the preview: eleven attachments must not mean eleven
  // full-resolution decodes. The filmstrip gets thumbnails; full resolution is confined
  // to the picture on screen plus the two you could reach with one keypress.
  it("loads full resolution for the active picture and its neighbours only", async () => {
    render(<PreviewPanel />);
    pushState(2, 11);
    await screen.findByText("2 / 11");

    expect(screen.getByTestId("preview-stage-image").getAttribute("src")).toBe(
      "https://webview.local/blobs/full-2",
    );
    expect(
      screen
        .getAllByTestId("preview-neighbour-image")
        .map((element) => element.getAttribute("src")),
    ).toEqual([
      "https://webview.local/blobs/full-1",
      "https://webview.local/blobs/full-3",
    ]);

    const filmstrip = screen.getAllByTestId("preview-filmstrip-image");
    expect(filmstrip).toHaveLength(11);
    for (const [index, element] of filmstrip.entries()) {
      expect(element.getAttribute("src")).toBe(
        `https://webview.local/thumbs/thumb-${index + 1}`,
      );
      expect(element.getAttribute("loading")).toBe("lazy");
    }

    // Nothing anywhere in the panel may be an inline base64 payload.
    for (const element of screen.getAllByRole("img", { hidden: true })) {
      expect(element.getAttribute("src")).not.toContain("data:");
    }
  });
});
