import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ImageLightbox, type ZoomedImage } from "./ImageLightbox";
import { typedBlobUrl } from "../attachments/imagePipeline";
vi.mock("../attachments/imagePipeline", () => ({ typedBlobUrl: vi.fn() }));

const IMAGE: ZoomedImage = {
  alt: "diagram",
  src: "vscode-webview://workspace/docs/diagram.png",
};

describe("ImageLightbox", () => {
  it("uses the existing typed SVG loader, releases its URL on close and ignores late loads", async () => {
    const revoke = vi.fn(); vi.stubGlobal("URL", { revokeObjectURL: revoke });
    const image = { alt: "original svg", src: "https://resource/blobs/hash", mimeType: "image/svg+xml" };
    vi.mocked(typedBlobUrl).mockResolvedValueOnce("blob:typed-svg");
    const view = render(<ImageLightbox image={image} onClose={vi.fn()} />);
    await act(async () => { await Promise.resolve(); });
    expect(typedBlobUrl).toHaveBeenCalledWith(image.src, "image/svg+xml");
    expect(screen.getByTestId("image-lightbox-image").getAttribute("src")).toBe("blob:typed-svg");
    view.rerender(<ImageLightbox image={null} onClose={vi.fn()} />);
    expect(revoke).toHaveBeenCalledWith("blob:typed-svg");
    let resolve!: (value: string) => void;
    vi.mocked(typedBlobUrl).mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    view.rerender(<ImageLightbox image={image} onClose={vi.fn()} />);
    view.rerender(<ImageLightbox image={null} onClose={vi.fn()} />);
    await act(async () => { resolve("blob:late"); await Promise.resolve(); });
    expect(revoke).toHaveBeenCalledWith("blob:late");
    expect(screen.queryByRole("dialog")).toBeNull();
    vi.unstubAllGlobals();
  });
  it("shows an ordinary preview error instead of silently falling back to untyped SVG", async () => {
    vi.mocked(typedBlobUrl).mockRejectedValueOnce(new Error("missing blob"));
    render(<ImageLightbox image={{ ...IMAGE, mimeType: "image/svg+xml" }} onClose={vi.fn()} />);
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByRole("alert").textContent).toContain("Unable to load");
    expect(screen.queryByTestId("image-lightbox-image")).toBeNull();
  });
  it("renders nothing when closed", () => {
    const { container } = render(<ImageLightbox image={null} onClose={() => undefined} />);
    expect(container.childElementCount).toBe(0);
  });

  it("renders an accessible dialog and moves focus to the close button", () => {
    render(<ImageLightbox image={IMAGE} onClose={() => undefined} />);
    const dialog = screen.getByRole("dialog", { name: "Image preview" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.activeElement).toBe(screen.getByTestId("image-lightbox-close"));
  });

  it("restores focus to the previously active element when it closes", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();

    const { rerender, unmount } = render(
      <ImageLightbox image={IMAGE} onClose={() => undefined} />,
    );
    expect(document.activeElement).toBe(screen.getByTestId("image-lightbox-close"));

    rerender(<ImageLightbox image={null} onClose={() => undefined} />);
    expect(document.activeElement).toBe(trigger);

    trigger.focus();
    rerender(<ImageLightbox image={IMAGE} onClose={() => undefined} />);
    unmount();
    expect(document.activeElement).toBe(trigger);

    trigger.remove();
  });

  it("closes on overlay, dialog blank area, close button, and Escape", () => {
    const onClose = vi.fn();
    render(<ImageLightbox image={IMAGE} onClose={onClose} />);

    fireEvent.mouseDown(screen.getByTestId("image-lightbox-overlay"));
    fireEvent.mouseDown(screen.getByTestId("image-lightbox"));
    fireEvent.click(screen.getByTestId("image-lightbox-close"));
    fireEvent.keyDown(document, { key: "Escape" });

    expect(onClose).toHaveBeenCalledTimes(4);
  });

  it("does not close when clicking the enlarged image itself", () => {
    const onClose = vi.fn();
    render(<ImageLightbox image={IMAGE} onClose={onClose} />);

    fireEvent.mouseDown(screen.getByTestId("image-lightbox-image"));

    expect(onClose).not.toHaveBeenCalled();
  });
});
