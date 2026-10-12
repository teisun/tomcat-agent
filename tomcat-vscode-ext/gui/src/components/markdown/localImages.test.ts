import { describe, expect, it } from "vitest";

import { resolveLocalImageSrc, rewriteLocalImages } from "./localImages";
import type { WebviewMediaRoot } from "../../types";

const ROOTS: WebviewMediaRoot[] = [
  {
    fsPath: "/workspace",
    webviewBase: "vscode-webview://workspace",
  },
  {
    fsPath: "/tmp/tomcat-inline",
    webviewBase: "vscode-webview://tmp",
  },
];

describe("localImages", () => {
  it("accepts absolute and relative paths within a granted workspace root", () => {
    expect(resolveLocalImageSrc("/workspace/docs/diagram.png", ROOTS)).toBe(
      "vscode-webview://workspace/docs/diagram.png",
    );
    expect(resolveLocalImageSrc("docs/diagram.png", ROOTS)).toBe(
      "vscode-webview://workspace/docs/diagram.png",
    );
  });

  it("accepts normalized paths that stay within the root", () => {
    expect(resolveLocalImageSrc("docs/../docs/mockup.png", ROOTS)).toBe(
      "vscode-webview://workspace/docs/mockup.png",
    );
  });

  it("rejects traversal outside every granted root", () => {
    expect(resolveLocalImageSrc("../../../etc/passwd", ROOTS)).toBeNull();
    expect(resolveLocalImageSrc("/etc/passwd", ROOTS)).toBeNull();
  });

  it("treats /tmp and /private/tmp as the same granted temp directory", () => {
    const tmpRoots: WebviewMediaRoot[] = [
      {
        fsPath: "/tmp/tomcat-inline",
        webviewBase: "vscode-webview://tmp",
      },
    ];
    expect(resolveLocalImageSrc("/private/tmp/tomcat-inline/render.png", tmpRoots)).toBe(
      "vscode-webview://tmp/render.png",
    );
    expect(resolveLocalImageSrc("/tmp/tomcat-inline/render.png", tmpRoots)).toBe(
      "vscode-webview://tmp/render.png",
    );
  });

  it("rejects remote and browser-managed schemes", () => {
    expect(resolveLocalImageSrc("http://example.com/a.png", ROOTS)).toBeNull();
    expect(resolveLocalImageSrc("https://example.com/a.png", ROOTS)).toBeNull();
    expect(resolveLocalImageSrc("data:image/png;base64,abc", ROOTS)).toBeNull();
    expect(resolveLocalImageSrc("blob:https://example.com/id", ROOTS)).toBeNull();
  });

  it("encodes spaces, unicode, and # in filenames", () => {
    expect(resolveLocalImageSrc("assets/设计 #1.png", ROOTS)).toBe(
      "vscode-webview://workspace/assets/%E8%AE%BE%E8%AE%A1%20%231.png",
    );
  });

  it("rewrites allowed images in-place with a zoom payload", () => {
    const container = document.createElement("div");
    container.innerHTML = '<p><img alt="diagram" src="docs/diagram final.png"></p>';

    rewriteLocalImages(container, ROOTS);

    const image = container.querySelector("img");
    expect(image).not.toBeNull();
    expect(image?.classList.contains("tc-inline-image")).toBe(true);
    expect(image?.getAttribute("data-tc-image-src")).toBe(
      "vscode-webview://workspace/docs/diagram%20final.png",
    );
    expect(image?.getAttribute("src")).toBe(
      "vscode-webview://workspace/docs/diagram%20final.png",
    );
  });

  it("degrades blocked images into path links without throwing when no roots exist", () => {
    const container = document.createElement("div");
    container.innerHTML = '<p><img alt="remote" src="https://example.com/a.png"></p>';

    expect(() => rewriteLocalImages(container, [])).not.toThrow();

    expect(container.querySelector("img")).toBeNull();
    const blocked = container.querySelector<HTMLAnchorElement>(".tc-blocked-image");
    expect(blocked).not.toBeNull();
    expect(blocked?.textContent).toBe("https://example.com/a.png");
    expect(blocked?.getAttribute("href")).toBe("https://example.com/a.png");
  });

  it("degrades out-of-root local images into clickable file links", () => {
    const container = document.createElement("div");
    container.innerHTML = '<p><img alt="outside" src="/outside/secret.png"></p>';

    rewriteLocalImages(container, ROOTS);

    expect(container.querySelector("img")).toBeNull();
    const blocked = container.querySelector<HTMLAnchorElement>(".tc-blocked-image");
    expect(blocked?.getAttribute("href")).toBe("#");
    expect(blocked?.getAttribute("data-tc-file-path")).toBe("/outside/secret.png");
  });
});
