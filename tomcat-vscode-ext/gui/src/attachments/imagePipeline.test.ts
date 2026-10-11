import { describe, expect, it, vi } from "vitest";

import {
  fitWithin,
  makeThumbnail,
  rasterizeSvgToPng,
  prepareAttachment,
  svgSourceIsUninformative,
  svgWithExplicitSize,
  SVG_SOURCE_MAX_BYTES,
  THUMBNAIL_MAX_EDGE,
} from "./imagePipeline";

// jsdom has no raster decoder and no canvas backend, so the pixel-pushing paths are
// covered by the Dev Host e2e run instead. What is tested here is the logic that decides
// *what* to push: dimension derivation, aspect-preserving fit, and the degradation
// chain. Those are exactly the parts that were wrong in the first draft.

const encoder = new TextEncoder();

function bytesOf(text: string): ArrayBuffer {
  const encoded = encoder.encode(text);
  return encoded.buffer.slice(
    encoded.byteOffset,
    encoded.byteOffset + encoded.byteLength,
  ) as ArrayBuffer;
}

describe("svgWithExplicitSize", () => {
  it("keeps declared width and height untouched", () => {
    const source =
      '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="36"><rect/></svg>';
    const result = svgWithExplicitSize(source);
    expect(result.width).toBe(24);
    expect(result.height).toBe(36);
    expect(result.source).toBe(source);
  });

  it("derives size from viewBox when width and height are missing", () => {
    // The common case: design tools export viewBox-only SVGs, and without explicit
    // dimensions Chromium reports naturalWidth 0 and drawImage draws nothing.
    const result = svgWithExplicitSize(
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 60"><rect/></svg>',
    );
    expect(result.width).toBe(120);
    expect(result.height).toBe(60);
    expect(result.source).toContain('width="120"');
    expect(result.source).toContain('height="60"');
  });

  it("handles a viewBox with comma separators and a negative origin", () => {
    const result = svgWithExplicitSize('<svg viewBox="-10,-10,200,100"><rect/></svg>');
    expect(result.width).toBe(200);
    expect(result.height).toBe(100);
  });

  it("falls back to a square when the SVG declares no size at all", () => {
    const result = svgWithExplicitSize("<svg><rect/></svg>");
    expect(result.width).toBe(512);
    expect(result.height).toBe(512);
    expect(result.source).toContain('width="512"');
  });

  it.each([
    '<svg width="80" viewBox="0 0 80 40"><rect/></svg>',
    '<svg height="40" viewBox="0 0 80 40"><rect/></svg>',
  ])("fills in only the missing dimension without duplicating XML attributes: %s", (source) => {
    const result = svgWithExplicitSize(source);
    expect(result.width).toBe(80);
    expect(result.height).toBe(40);
    const document = new DOMParser().parseFromString(result.source, "image/svg+xml");
    expect(document.querySelector("parsererror")).toBeNull();
    expect(document.documentElement.getAttribute("width")).toBe("80");
    expect(document.documentElement.getAttribute("height")).toBe("40");
  });

  it("does not choke on a real design-tool export", () => {
    // Everything in here (style=, <style>, url(#grad)) was rejected outright by the
    // blacklist this replaces. It must now sail through.
    const source = [
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">',
      '<defs><linearGradient id="grad"><stop offset="0" stop-color="#f00"/></linearGradient></defs>',
      "<style>.icon { stroke-width: 2 }</style>",
      '<rect class="icon" style="opacity:0.8" fill="url(#grad)" width="24" height="24"/>',
      "</svg>",
    ].join("");
    const result = svgWithExplicitSize(source);
    expect(result.width).toBe(24);
    expect(result.height).toBe(24);
  });
});

describe("fitWithin", () => {
  it("leaves an already-small image alone", () => {
    expect(fitWithin(100, 80, THUMBNAIL_MAX_EDGE)).toEqual({ height: 80, width: 100 });
  });

  it("scales a landscape image by its longest edge", () => {
    expect(fitWithin(4000, 3000, 192)).toEqual({ height: 144, width: 192 });
  });

  it("scales a portrait image by its longest edge, preserving aspect", () => {
    // Passing resizeWidth alone would have squashed this one; the ratio must survive.
    expect(fitWithin(3000, 4000, 192)).toEqual({ height: 192, width: 144 });
  });

  it("never produces a zero dimension for extreme aspect ratios", () => {
    const result = fitWithin(10000, 3, 192);
    expect(result.width).toBe(192);
    expect(result.height).toBeGreaterThanOrEqual(1);
  });

  it("tolerates a zero-sized source", () => {
    expect(fitWithin(0, 0, 192)).toEqual({ height: 1, width: 1 });
  });
});

describe("svgSourceIsUninformative", () => {
  it("flags an SVG that just wraps an embedded bitmap", () => {
    expect(
      svgSourceIsUninformative(
        '<svg><image href="data:image/png;base64,AAAA" width="10" height="10"/></svg>',
      ),
    ).toBe(true);
  });

  it("also catches the xlink namespace spelling", () => {
    expect(
      svgSourceIsUninformative('<svg><image xlink:href="data:image/jpeg;base64,AAAA"/></svg>'),
    ).toBe(true);
  });

  it("does not flag real vector content", () => {
    expect(svgSourceIsUninformative('<svg><path d="M0 0 L10 10"/></svg>')).toBe(false);
  });
});

describe("prepareAttachment degradation", () => {
  it("preserves the original sourcePath as display metadata", async () => {
    const prepared = await prepareAttachment({
      bytes: bytesOf("%PDF"),
      filename: "brief.pdf",
      mimeType: "application/pdf",
      sourcePath: "/workspace/docs/brief.pdf",
    });

    expect(prepared.sourcePath).toBe("/workspace/docs/brief.pdf");
  });

  it("always returns the original bytes even when every derived artefact fails", async () => {
    // jsdom cannot rasterise, so this run exercises the all-failures path for free.
    const source = '<svg viewBox="0 0 10 10"><path d="M0 0 L10 10"/></svg>';
    const prepared = await prepareAttachment({
      bytes: bytesOf(source),
      filename: "icon.svg",
      mimeType: "image/svg+xml",
      sourcePath: "/workspace/icons/icon.svg",
    });

    expect(atob(prepared.dataBase64)).toBe(source);
    expect(prepared.mimeType).toBe("image/svg+xml");
    expect(prepared.sourcePath).toBe("/workspace/icons/icon.svg");
    expect(prepared.warnings.length).toBeGreaterThan(0);
  });

  it("falls back to sending SVG source as text when rasterisation fails", async () => {
    const source = '<svg viewBox="0 0 10 10"><path d="M0 0 L10 10"/></svg>';
    const prepared = await prepareAttachment({
      bytes: bytesOf(source),
      filename: "icon.svg",
      mimeType: "image/svg+xml",
    });

    expect(prepared.providerText).toBe(source);
    expect(prepared.providerBase64).toBeUndefined();
    expect(prepared.warnings.join(" ")).toContain("source code");
  });

  it("refuses to inline SVG source past the size cap", async () => {
    const source = `<svg viewBox="0 0 10 10">${"<path d='M0 0'/>".repeat(
      Math.ceil(SVG_SOURCE_MAX_BYTES / 15),
    )}</svg>`;
    const prepared = await prepareAttachment({
      bytes: bytesOf(source),
      filename: "huge.svg",
      mimeType: "image/svg+xml",
    });

    expect(prepared.providerText).toBeUndefined();
    expect(prepared.warnings.join(" ")).toContain("too large");
  });

  it("says so plainly when an SVG only wraps a bitmap", async () => {
    const prepared = await prepareAttachment({
      bytes: bytesOf('<svg><image href="data:image/png;base64,AAAA"/></svg>'),
      filename: "wrapper.svg",
      mimeType: "image/svg+xml",
    });

    expect(prepared.providerText).toBeUndefined();
    expect(prepared.warnings.join(" ")).toContain("embedded bitmap");
  });

  it("does not invent a provider rendering for ordinary bitmaps", async () => {
    // PNG and JPEG go to the provider as-is; only SVG needs a conversion.
    const prepared = await prepareAttachment({
      bytes: bytesOf("not really a png"),
      filename: "photo.png",
      mimeType: "image/png",
    });

    expect(prepared.providerBase64).toBeUndefined();
    expect(prepared.providerText).toBeUndefined();
  });

  it("records a warning rather than throwing when the thumbnail cannot be made", async () => {
    const prepared = await prepareAttachment({
      bytes: bytesOf("not really a png"),
      filename: "photo.png",
      mimeType: "image/png",
    });

    expect(prepared.thumbBase64).toBeUndefined();
    expect(prepared.warnings.join(" ")).toContain("thumbnail unavailable");
  });

  it.each([
    { mimeType: "image/png", expected: [192, 144] },
    { mimeType: "image/svg+xml", expected: [2048, 1536] },
  ])("bounds application-owned canvases without a full-size bitmap probe: $mimeType", async ({ mimeType, expected }) => {
    const close = vi.fn();
    const bitmap = vi.fn(async (_blob: Blob, options?: ImageBitmapOptions) => ({
      close,
      height: options?.resizeHeight ?? 3000,
      width: options?.resizeWidth ?? 4000,
    }));
    vi.stubGlobal("createImageBitmap", bitmap);
    vi.stubGlobal("URL", {
      createObjectURL: vi.fn(() => "blob:test"),
      revokeObjectURL: vi.fn(),
    });
    vi.stubGlobal("Image", class {
      naturalWidth = 4000;
      naturalHeight = 3000;
      onload: (() => void) | null = null;
      set src(_value: string) { queueMicrotask(() => this.onload?.()); }
    });
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
    const dimensions: number[][] = [];
    vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(function (this: HTMLCanvasElement, callback: BlobCallback) {
      dimensions.push([this.width, this.height]);
      callback(new Blob(["png"], { type: "image/png" }));
    });
    try {
      const bytes = bytesOf('<svg width="4000" height="3000"/>');
      const result = mimeType === "image/svg+xml"
        ? await rasterizeSvgToPng(bytes)
        : await makeThumbnail(bytes, mimeType);
      expect(result.type).toBe("image/png");
      expect(dimensions).toEqual([expected]);
      expect(bitmap).not.toHaveBeenCalled();
    } finally {
      vi.restoreAllMocks();
      vi.unstubAllGlobals();
    }
  });
});
