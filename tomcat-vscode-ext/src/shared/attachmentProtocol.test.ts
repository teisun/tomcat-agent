import { describe, expect, it } from "vitest";

import {
  IMAGE_MAX_BYTES,
  PDF_MAX_BYTES,
  decodeBase64Strict,
  safeAttachmentFilename,
  validateAttachmentCandidate,
} from "./attachmentProtocol";

describe("attachmentProtocol utilities", () => {
  it("strictly decodes canonical base64 and rejects malformed input", () => {
    expect(decodeBase64Strict(Buffer.from("hello").toString("base64"))?.toString()).toBe(
      "hello",
    );
    expect(decodeBase64Strict("not base64")).toBeNull();
    expect(decodeBase64Strict("YQ=")).toBeNull();
    expect(decodeBase64Strict("")).toBeNull();
  });

  it("normalizes unsafe filenames to a basename or fallback", () => {
    expect(safeAttachmentFilename("../../screen.png", "fallback.png")).toBe(
      "screen.png",
    );
    expect(safeAttachmentFilename("..", "fallback.png")).toBe("fallback.png");
    expect(safeAttachmentFilename("\u0000shot.png", "fallback.png")).toBe(
      "shot.png",
    );
  });

  // Regression guard for the blacklist this replaced. It refused `style=`, `<style>`
  // and `url(`, which is to say it refused nearly everything Figma, Illustrator and
  // Sketch export — while a namespace-aliased `x:href` sailed past it. Design-tool SVG
  // must be attachable; the security story is `<img>` secure static mode, asserted in
  // the webview suite instead.
  it("accepts real design-tool SVG that the old text blacklist rejected", () => {
    const designToolSvgs = [
      '<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:red" width="8" height="8"/></svg>',
      '<svg xmlns="http://www.w3.org/2000/svg"><style>.a{fill:#0af}</style><rect class="a" width="8" height="8"/></svg>',
      '<svg xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="grad"/></defs><rect fill="url(#grad)" width="8" height="8"/></svg>',
    ];
    for (const svg of designToolSvgs) {
      expect(
        validateAttachmentCandidate({
          dataBase64: Buffer.from(svg).toString("base64"),
          filename: "icon.svg",
          mimeType: "image/svg+xml",
        }),
      ).toMatchObject({ filename: "icon.svg", ok: true });
    }
  });

  it("validates MIME, base64 shape and decoded byte size", () => {
    const png = validateAttachmentCandidate({
      dataBase64: Buffer.from([1, 2, 3]).toString("base64"),
      filename: null,
      mimeType: "image/png",
    });
    expect(png).toMatchObject({ filename: "pasted-image.png", kind: "image", ok: true });

    const pdf = validateAttachmentCandidate({
      dataBase64: Buffer.from("%PDF-1.7").toString("base64"),
      filename: null,
      mimeType: "application/pdf",
    });
    expect(pdf).toMatchObject({
      filename: "attachment.pdf",
      kind: "file",
      mimeType: "application/pdf",
      ok: true,
    });

    expect(
      validateAttachmentCandidate({
        dataBase64: Buffer.from([1]).toString("base64"),
        mimeType: "image/bmp",
      }),
    ).toMatchObject({ error: "unsupported type image/bmp", ok: false });

    expect(
      validateAttachmentCandidate({
        dataBase64: Buffer.alloc(IMAGE_MAX_BYTES + 1).toString("base64"),
        mimeType: "image/png",
      }),
    ).toMatchObject({ error: "exceeds 4.5 MB", ok: false });

    expect(
      validateAttachmentCandidate({
        dataBase64: Buffer.alloc(PDF_MAX_BYTES + 1).toString("base64"),
        mimeType: "application/pdf",
      }),
    ).toMatchObject({ error: "PDF exceeds 25 MB", ok: false });
  }, 15_000);
});
