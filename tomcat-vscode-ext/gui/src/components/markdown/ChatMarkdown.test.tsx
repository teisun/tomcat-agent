import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChatMarkdown } from "./ChatMarkdown";
import { LocaleProvider } from "../../i18n/LocaleProvider";
import { translate } from "../../../../src/shared/i18n";
import * as richRenderRuntime from "./richRenderRuntime";

const renderMock = vi.fn(async (_id: string, _graph: string) => ({
  svg: '<svg data-testid="mermaid-svg"><g>flow</g></svg>',
}));
const initializeMock = vi.fn();
const ROOTS = [
  {
    fsPath: "/workspace",
    webviewBase: "vscode-webview://workspace",
  },
];

vi.mock("mermaid", () => ({
  default: {
    initialize: initializeMock,
    render: renderMock,
  },
}));

describe("ChatMarkdown", () => {
  it("switches copy feedback language without rebuilding code nodes or translating their contents", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(globalThis.navigator, { clipboard: { writeText } });
    const markdown = "```ts\nconst same = 1;\n```";
    const open = vi.fn();
    const view = render(<LocaleProvider locale="en"><ChatMarkdown markdown={markdown} onOpenFile={open} /></LocaleProvider>);
    try {
      const card = screen.getByTestId("assistant-code-card");
      const code = card.querySelector("code");
      const copy = screen.getByRole("button", { name: translate("en", "code.copy") });
      await act(async () => { fireEvent.click(copy); await Promise.resolve(); });
      expect(copy.getAttribute("aria-label")).toBe(translate("en", "common.copied"));
      view.rerender(<LocaleProvider locale="zh-CN"><ChatMarkdown markdown={markdown} onOpenFile={open} /></LocaleProvider>);
      expect(screen.getByTestId("assistant-code-copy")).toBe(copy);
      expect(copy.classList.contains("is-copied")).toBe(true);
      expect(screen.getByTestId("assistant-code-card")).toBe(card);
      expect(card.querySelector("code")).toBe(code);
      expect(code?.textContent).toBe("const same = 1;\n");
      await act(async () => { vi.advanceTimersByTime(1500); });
      expect(copy.classList.contains("is-copied")).toBe(false);
      expect(screen.getByTestId("assistant-code-copy")).toBe(copy);
      expect(writeText).toHaveBeenCalledExactlyOnceWith("const same = 1;\n");
    } finally { vi.useRealTimers(); }
  });

  it("renders headings, lists, bold and inline code from assistant markdown", () => {
    render(
      <ChatMarkdown
        markdown={"## Title\n\nA **bold** word and `inline`.\n\n- one\n- two"}
        onOpenFile={() => undefined}
      />,
    );
    const body = screen.getByTestId("chat-markdown");
    expect(body.querySelector("h2")?.textContent).toBe("Title");
    expect(body.querySelector("strong")?.textContent).toBe("bold");
    expect(body.querySelectorAll("li")).toHaveLength(2);
    expect(body.querySelector("code")?.textContent).toBe("inline");
  });

  it("renders fenced code as a code card and opens the file from the header", async () => {
    const onOpenFile = vi.fn();
    render(
      <ChatMarkdown
        markdown={"```rust src/core/foo.rs:42\nfn main() {}\n```\n"}
        onOpenFile={onOpenFile}
      />,
    );

    const card = screen.getByTestId("assistant-code-card");
    expect(card.querySelector(".tc-code-card__header")).not.toBeNull();
    expect(card.querySelector(".tc-code-card__lang")).toBeNull();

    const fileButton = screen.getByTestId("assistant-code-file");
    expect(fileButton.textContent).toContain("foo.rs:42");
    expect(fileButton.textContent).not.toContain("src/core/");
    expect(fileButton.getAttribute("title")).toBe("src/core/foo.rs:42");

    fireEvent.click(fileButton);
    expect(onOpenFile).toHaveBeenCalledWith("src/core/foo.rs", 42);
  });

  it("renders no-path fences as bare cards with icon-only copy feedback", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(globalThis.navigator, {
      clipboard: {
        writeText,
      },
    });

    render(
      <ChatMarkdown
        markdown={"```ts\nconst answer = 42;\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    try {
      const card = screen.getByTestId("assistant-code-card");
      expect(card.classList.contains("tc-code-card--bare")).toBe(true);
      expect(card.querySelector(".tc-code-card__header")).toBeNull();

      const copyButton = screen.getByTestId("assistant-code-copy");
      expect(copyButton.textContent).toBe("");
      expect(copyButton.getAttribute("aria-label")).toBe("Copy code");
      expect(copyButton.querySelector(".codicon-copy")).not.toBeNull();

      await act(async () => {
        fireEvent.click(copyButton);
        await Promise.resolve();
      });

      expect(writeText).toHaveBeenCalledWith("const answer = 42;\n");
      expect(copyButton.classList.contains("is-copied")).toBe(true);
      expect(copyButton.querySelector(".codicon-check")).not.toBeNull();

      await act(async () => {
        vi.advanceTimersByTime(1_500);
      });

      expect(copyButton.classList.contains("is-copied")).toBe(false);
      expect(copyButton.querySelector(".codicon-copy")).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("linkifies inline file paths and forwards clicks with the parsed line number", async () => {
    const onOpenFile = vi.fn();
    const resolvePaths = vi.fn().mockResolvedValue([
      {
        kind: "file",
        path: "src/gui/App.tsx",
        resolvedPath: "/workspace/src/gui/App.tsx",
      },
    ]);
    render(
      <ChatMarkdown
        markdown={"Check `src/gui/App.tsx:18` before editing."}
        onOpenFile={onOpenFile}
        resolvePaths={resolvePaths}
      />,
    );

    expect(screen.getByTestId("chat-markdown").querySelector("code")?.textContent).toBe(
      "src/gui/App.tsx:18",
    );
    const link = await screen.findByTestId("assistant-clickable-path");
    expect(link.textContent).toContain("App.tsx:18");
    expect(link.textContent).not.toContain("src/gui/");
    expect(link.getAttribute("title")).toBe("src/gui/App.tsx:18");
    fireEvent.click(link);
    expect(onOpenFile).toHaveBeenCalledWith("/workspace/src/gui/App.tsx", 18);
  });

  it("keeps unresolved path-shaped code as ordinary inline code", async () => {
    render(
      <ChatMarkdown
        markdown={"Do not link `*.plan.md`."}
        onOpenFile={() => undefined}
        resolvePaths={vi.fn().mockResolvedValue([
          { kind: "missing", path: "*.plan.md", resolvedPath: "/workspace/*.plan.md" },
        ])}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("chat-markdown").querySelector(".tc-inline-path")).toBeNull();
      expect(screen.getByTestId("chat-markdown").querySelector("code")?.textContent).toBe(
        "*.plan.md",
      );
    });
  });

  it("ignores a stale path-resolution result after the Markdown changes", async () => {
    const pending = new Map<string, (results: Array<{
      kind: "file";
      path: string;
      resolvedPath: string;
    }>) => void>();
    const resolvePaths = vi.fn(
      (paths: string[]) =>
        new Promise<Array<{ kind: "file"; path: string; resolvedPath: string }>>((resolve) => {
          pending.set(paths[0], resolve);
        }),
    );
    const { rerender } = render(
      <ChatMarkdown
        markdown={"Open `src/old.ts`."}
        onOpenFile={() => undefined}
        resolvePaths={resolvePaths}
      />,
    );
    await waitFor(() => {
      expect(resolvePaths).toHaveBeenCalledWith(["src/old.ts"]);
    });
    rerender(
      <ChatMarkdown
        markdown={"Open `src/new.ts`."}
        onOpenFile={() => undefined}
        resolvePaths={resolvePaths}
      />,
    );
    await waitFor(() => {
      expect(resolvePaths).toHaveBeenCalledWith(["src/new.ts"]);
    });

    pending.get("src/old.ts")?.([
      { kind: "file", path: "src/old.ts", resolvedPath: "/workspace/src/old.ts" },
    ]);
    pending.get("src/new.ts")?.([
      { kind: "file", path: "src/new.ts", resolvedPath: "/workspace/src/new.ts" },
    ]);

    const link = await screen.findByTestId("assistant-clickable-path");
    expect(link.textContent).toBe("new.ts");
    expect(screen.queryByText("old.ts")).toBeNull();
  });

  it("forwards ordinary anchor clicks to onOpenLink", () => {
    const onOpenLink = vi.fn();
    render(
      <ChatMarkdown
        markdown={"See [the docs](https://example.com/docs)."}
        onOpenFile={() => undefined}
        onOpenLink={onOpenLink}
      />,
    );

    const link = screen.getByTestId("chat-markdown").querySelector("a") as HTMLAnchorElement | null;
    expect(link?.getAttribute("href")).toBe("https://example.com/docs");
    fireEvent.click(link!);
    expect(onOpenLink).toHaveBeenCalledWith("https://example.com/docs");
  });

  it("keeps non-path inline code as plain code", () => {
    render(
      <ChatMarkdown
        markdown={"The variable is `answer`, not a file path."}
        onOpenFile={() => undefined}
      />,
    );

    const body = screen.getByTestId("chat-markdown");
    expect(body.querySelector(".tc-inline-path")).toBeNull();
    expect(body.querySelector("code")?.textContent).toBe("answer");
  });

  it("sanitizes unsafe html before rendering", () => {
    render(
      <ChatMarkdown
        markdown={"safe\n\n<script>window.__tc_pwned = true;</script>\n\n<img src=x onerror=\"window.__tc_pwned = true\">"}
        onOpenFile={() => undefined}
      />,
    );
    const body = screen.getByTestId("chat-markdown");
    expect(body.querySelector("script")).toBeNull();
    expect(body.querySelector("img")).toBeNull();
    expect(body.querySelector(".tc-blocked-image")?.getAttribute("onerror")).toBeNull();
    expect((window as Window & { __tc_pwned?: boolean }).__tc_pwned).toBeUndefined();
  });

  it("renders allowed local markdown images with zoom metadata and forwards clicks", () => {
    const onZoomImage = vi.fn();
    render(
      <ChatMarkdown
        markdown={"![mockup](docs/mockup.png)"}
        mediaRoots={ROOTS}
        onOpenFile={() => undefined}
        onZoomImage={onZoomImage}
      />,
    );

    const image = screen
      .getByTestId("chat-markdown")
      .querySelector<HTMLImageElement>(".tc-inline-image");
    expect(image).not.toBeNull();
    expect(image?.getAttribute("data-tc-image-src")).toBe("vscode-webview://workspace/docs/mockup.png");

    fireEvent.click(image!);
    expect(onZoomImage).toHaveBeenCalledWith({
      alt: "mockup",
      src: "vscode-webview://workspace/docs/mockup.png",
    });
  });

  it("degrades remote markdown images into blocked links without rendering an img tag", () => {
    render(
      <ChatMarkdown
        markdown={"![remote](https://example.com/diagram.png)"}
        mediaRoots={ROOTS}
        onOpenFile={() => undefined}
      />,
    );

    const body = screen.getByTestId("chat-markdown");
    expect(body.querySelector("img")).toBeNull();
    const blocked = body.querySelector<HTMLAnchorElement>(".tc-blocked-image");
    expect(blocked).not.toBeNull();
    expect(blocked?.textContent).toBe("https://example.com/diagram.png");
  });

  it("auto-closes an unterminated fence so streaming partial code still renders as a card", async () => {
    render(
      <ChatMarkdown
        markdown={"```ts\nconst answer = 42;"}
        onOpenFile={() => undefined}
      />,
    );

    const card = screen.getByTestId("assistant-code-card");
    expect(card.classList.contains("tc-code-card--bare")).toBe(true);
    expect(card.textContent).toContain("const answer = 42;");
  });

  it("adds syntax highlighting without adding a header to no-path code fences", async () => {
    render(
      <ChatMarkdown
        markdown={"```ts\nconst answer = 42;\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    const card = screen.getByTestId("assistant-code-card");
    expect(card.classList.contains("tc-code-card--bare")).toBe(true);
    expect(card.querySelector(".tc-code-card__header")).toBeNull();
    const code = card.querySelector("code.hljs");
    expect(code).not.toBeNull();
    expect(code?.textContent).toBe("const answer = 42;\n");
  });

  it("renders mermaid fences into inline diagrams without overriding chat typography", async () => {
    renderMock.mockClear();
    initializeMock.mockClear();
    render(
      <ChatMarkdown
        markdown={"```mermaid\nflowchart LR\n  a --> b\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    const figure = await screen.findByTestId("plan-mermaid");
    expect(figure.querySelector("svg")).not.toBeNull();
    expect(renderMock).toHaveBeenCalledTimes(1);
    expect(initializeMock).toHaveBeenCalledWith(
      expect.not.objectContaining({ fontSize: expect.anything() }),
    );
  });

  it("highlights code synchronously while leaving mermaid rendering async", async () => {
    renderMock.mockClear();
    const { rerender } = render(
      <ChatMarkdown
        markdown={"```ts\nconst answer ="}
        onOpenFile={() => undefined}
      />,
    );

    const initialCode = screen.getByTestId("chat-markdown").querySelector("code.hljs");
    expect(initialCode).not.toBeNull();
    expect(initialCode?.textContent).toContain("const answer =");

    rerender(
      <ChatMarkdown
        markdown={"```ts\nconst answer = 42;\n```\n\n```mermaid\nflowchart LR\n  start --> finish\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    expect(screen.getByTestId("chat-markdown").querySelector("code.hljs")).not.toBeNull();
    expect(screen.queryByTestId("plan-mermaid")).toBeNull();

    const figure = await screen.findByTestId("plan-mermaid");
    expect(figure.querySelector("svg")).not.toBeNull();
    expect(renderMock).toHaveBeenCalledTimes(1);
  });

  it("only re-highlights a newly appended tail code block", () => {
    const spy = vi.spyOn(richRenderRuntime, "highlightToHtml");
    const { rerender } = render(
      <ChatMarkdown
        markdown={"```ts\nconst first = 1;\n```\n\nTail paragraph."}
        onOpenFile={() => undefined}
      />,
    );

    expect(spy.mock.calls.filter(([code]) => code.includes("const first = 1;"))).toHaveLength(1);

    rerender(
      <ChatMarkdown
        markdown={"```ts\nconst first = 1;\n```\n\nTail paragraph.\n\n```ts\nconst second = 2;\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    expect(spy.mock.calls.filter(([code]) => code.includes("const first = 1;"))).toHaveLength(1);
    expect(spy.mock.calls.filter(([code]) => code.includes("const second = 2;"))).toHaveLength(1);
    spy.mockRestore();
  });

  it("does not emit an img tag while a streaming image markdown token is still incomplete", () => {
    const { rerender } = render(
      <ChatMarkdown
        markdown={"![mockup](docs/mockup"}
        mediaRoots={ROOTS}
        onOpenFile={() => undefined}
      />,
    );

    const body = screen.getByTestId("chat-markdown");
    expect(body.querySelector("img")).toBeNull();

    rerender(
      <ChatMarkdown
        markdown={"![mockup](docs/mockup.png"}
        mediaRoots={ROOTS}
        onOpenFile={() => undefined}
      />,
    );
    expect(body.querySelector("img")).toBeNull();

    rerender(
      <ChatMarkdown
        markdown={"![mockup](docs/mockup.png)"}
        mediaRoots={ROOTS}
        onOpenFile={() => undefined}
      />,
    );
    expect(body.querySelector(".tc-inline-image")).not.toBeNull();
  });
});
