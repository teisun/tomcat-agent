import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WebviewPendingAttachment } from "../types";
import { AttachmentStrip } from "./AttachmentStrip";

function sha(seed: string): string {
  return seed.padEnd(64, "0").slice(0, 64);
}

function image(id: string): WebviewPendingAttachment {
  return {
    blobSha: sha(id),
    bytes: 4_500_000,
    filename: `${id}.png`,
    fullUri: `https://webview.local/blobs/${sha(id)}`,
    hasThumb: true,
    id,
    kind: "image",
    label: `${id}.png`,
    mimeType: "image/png",
    path: null,
    thumbUri: `https://webview.local/thumbs/${sha(id)}`,
  };
}

function pdf(overrides: Partial<WebviewPendingAttachment> = {}): WebviewPendingAttachment {
  return {
    blobSha: sha("pdf"),
    bytes: 1024,
    filename: "brief.pdf",
    id: "pdf-1",
    kind: "file",
    label: "brief.pdf",
    mimeType: "application/pdf",
    path: null,
    ...overrides,
  };
}

function fileAttachment(
  overrides: Partial<WebviewPendingAttachment> = {},
): WebviewPendingAttachment {
  return {
    blobSha: sha("file"),
    bytes: 2048,
    filename: "spec.docx",
    id: "file-1",
    kind: "file",
    label: "spec.docx",
    mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    path: null,
    ...overrides,
  };
}

describe("AttachmentStrip", () => {
  it("renders nothing for an empty collection", () => {
    const { container } = render(<AttachmentStrip attachments={[]} />);
    expect(container.innerHTML).toBe("");
  });

  it("renders image thumbnails and PDF chips in one list", () => {
    render(<AttachmentStrip attachments={[image("one"), pdf()]} />);
    expect(screen.getByTestId("attachment-thumb").getAttribute("aria-label")).toBe(
      "Open one.png",
    );
    const chip = screen.getByTestId("attachment-chip");
    expect(chip.textContent).toBe("PDF");
    expect(screen.queryByText("brief.pdf")).toBeNull();
    expect(chip.querySelector(".codicon-file-pdf")).toBeTruthy();
    expect(chip.getAttribute("title")).toBe("brief.pdf · 1 KB");
    expect(chip.tagName).toBe("SPAN");
    expect(screen.getByRole("list").getAttribute("data-attachment-source")).toBe(
      "draft",
    );
  });

  it("keeps open and remove actions independent", () => {
    const onOpen = vi.fn();
    const onRemove = vi.fn();
    render(
      <AttachmentStrip
        attachments={[image("one")]}
        onOpen={onOpen}
        onRemove={onRemove}
      />,
    );
    fireEvent.click(screen.getByTestId("attachment-remove"));
    expect(onRemove).toHaveBeenCalledWith("one");
    expect(onOpen).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("attachment-thumb"));
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: "one" }));
  });

  it("renders a clickable PDF chip with the full path as tooltip when available", () => {
    const onOpen = vi.fn();
    render(
      <AttachmentStrip
        attachments={[pdf({ path: "/tmp/brief.pdf" })]}
        onOpen={onOpen}
      />,
    );
    const chip = screen.getByTestId("attachment-chip");
    expect(chip.tagName).toBe("BUTTON");
    expect(chip.getAttribute("title")).toBe("/tmp/brief.pdf");
    fireEvent.click(chip);
    expect(onOpen).toHaveBeenCalledWith(
      expect.objectContaining({ id: "pdf-1", path: "/tmp/brief.pdf" }),
    );
  });

  it("derives a DOCX chip label from the filename extension", () => {
    render(<AttachmentStrip attachments={[fileAttachment()]} />);
    const chip = screen.getByTestId("attachment-chip");
    expect(chip.textContent).toBe("DOCX");
    expect(chip.querySelector(".codicon-file")).toBeTruthy();
  });

  it("falls back to FILE when the filename has no extension", () => {
    render(
      <AttachmentStrip
        attachments={[
          fileAttachment({
            filename: "README",
            label: "README",
            mimeType: "application/octet-stream",
          }),
        ]}
      />,
    );
    expect(screen.getByTestId("attachment-chip").textContent).toBe("FILE");
  });

  it("renders eleven read-only history images without delete controls", () => {
    render(
      <AttachmentStrip
        attachments={Array.from({ length: 11 }, (_, index) => image(`i-${index}`))}
        readonly
      />,
    );
    expect(screen.getAllByTestId("history-attachment-thumb")).toHaveLength(11);
    expect(screen.queryByTestId("attachment-remove")).toBeNull();
    expect(screen.getByRole("list").getAttribute("data-attachment-source")).toBe(
      "history",
    );
  });

  // The memory contract, as an assertion rather than a comment. Eleven 4.5MB photos
  // rendered at 48px must not cause eleven full-resolution decodes, and no image may
  // arrive as base64 — that combination is what made the strip cost hundreds of
  // megabytes.
  it("loads only thumbnails, lazily, and never inline bytes", () => {
    const attachments = Array.from({ length: 11 }, (_, index) => image(`i-${index}`));
    render(<AttachmentStrip attachments={attachments} />);
    const images = screen.getAllByRole("img");
    expect(images).toHaveLength(11);
    for (const [index, element] of images.entries()) {
      expect(element.getAttribute("src")).toBe(attachments[index]!.thumbUri);
      expect(element.getAttribute("src")).not.toContain("data:");
      expect(element.getAttribute("loading")).toBe("lazy");
      expect(element.getAttribute("decoding")).toBe("async");
    }
  });

  it("marks an attachment whose bytes are gone and still allows removing it", () => {
    const onRemove = vi.fn();
    render(
      <AttachmentStrip
        attachments={[{ ...image("gone"), unavailable: true }]}
        onRemove={onRemove}
      />,
    );
    const chip = screen.getByTestId("attachment-unavailable");
    expect(chip.textContent).toContain("gone.png");
    // A screen reader gets the whole story, not just a filename with a warning glyph.
    expect(chip.getAttribute("aria-label")).toBe(
      "gone.png: image data is no longer available. Remove it and add the image again.",
    );
    expect(screen.queryByRole("img")).toBeNull();
    fireEvent.click(screen.getByTestId("attachment-remove"));
    expect(onRemove).toHaveBeenCalledWith("gone");
  });

  // Stands in for the E2E screenshot this state cannot get: thumbnails are generated
  // before an attachment reaches the strip, so there is no real sequence where one is
  // pending long enough to photograph. What matters is the size match, which is what
  // keeps the composer from jumping as images land.
  it("holds a placeholder of the final thumbnail size while one is missing", () => {
    render(<AttachmentStrip attachments={[{ ...image("pending"), thumbUri: undefined }]} />);
    const skeleton = screen.getByTestId("attachment-skeleton");
    expect(skeleton.getAttribute("aria-label")).toBe("pending.png is loading");
    // No broken image and no misleading file chip in place of a picture.
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.queryByTestId("attachment-chip")).toBeNull();
  });

  it("removes a focused attachment on Delete, since clicking one opens the preview", () => {
    const onOpen = vi.fn();
    const onRemove = vi.fn();
    render(
      <AttachmentStrip
        attachments={[image("one")]}
        onOpen={onOpen}
        onRemove={onRemove}
      />,
    );
    const thumb = screen.getByTestId("attachment-thumb");
    fireEvent.keyDown(thumb, { key: "Delete" });
    expect(onRemove).toHaveBeenCalledWith("one");
    fireEvent.keyDown(thumb, { key: "Backspace" });
    expect(onRemove).toHaveBeenCalledTimes(2);
    // Other keys stay with the button's own behaviour.
    fireEvent.keyDown(thumb, { key: "a" });
    expect(onRemove).toHaveBeenCalledTimes(2);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("never offers keyboard removal for read-only history images", () => {
    const onRemove = vi.fn();
    render(
      <AttachmentStrip attachments={[image("one")]} onRemove={onRemove} readonly />,
    );
    fireEvent.keyDown(screen.getByTestId("history-attachment-thumb"), {
      key: "Delete",
    });
    expect(onRemove).not.toHaveBeenCalled();
  });

  it("keeps PDF chips visually consistent between draft and readonly history modes", () => {
    const withPath = render(
      <AttachmentStrip attachments={[pdf({ path: "/tmp/brief.pdf" })]} />,
    );
    expect(withPath.getByTestId("attachment-chip").className).toContain(
      "tc-attachment-strip__file-chip",
    );
    withPath.unmount();

    render(
      <AttachmentStrip attachments={[pdf()]} readonly />,
    );
    expect(screen.getByTestId("history-attachment-chip").className).toContain(
      "tc-attachment-strip__file-chip",
    );
  });
});
