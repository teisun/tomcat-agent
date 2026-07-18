import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChatMarkdown } from "./ChatMarkdown";

const renderMock = vi.fn(async (_id: string, _graph: string) => ({
  svg: '<svg data-testid="mermaid-svg"><g>flow</g></svg>',
}));
const initializeMock = vi.fn();

vi.mock("mermaid", () => ({
  default: {
    initialize: initializeMock,
    render: renderMock,
  },
}));

describe("ChatMarkdown", () => {
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
    render(
      <ChatMarkdown
        markdown={"Check `src/gui/App.tsx:18` before editing."}
        onOpenFile={onOpenFile}
      />,
    );

    const link = screen.getByTestId("assistant-clickable-path");
    expect(link.textContent).toContain("App.tsx:18");
    expect(link.textContent).not.toContain("src/gui/");
    expect(link.getAttribute("title")).toBe("src/gui/App.tsx:18");
    fireEvent.click(link);
    expect(onOpenFile).toHaveBeenCalledWith("src/gui/App.tsx", 18);
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
    expect(body.querySelector("img")?.getAttribute("onerror")).toBeNull();
    expect((window as Window & { __tc_pwned?: boolean }).__tc_pwned).toBeUndefined();
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

    await waitFor(() => {
      const code = card.querySelector("code.hljs");
      expect(code).not.toBeNull();
      expect(code?.textContent).toBe("const answer = 42;\n");
    });
  });

  it("renders mermaid fences into inline diagrams", async () => {
    renderMock.mockClear();
    render(
      <ChatMarkdown
        markdown={"```mermaid\nflowchart LR\n  a --> b\n```\n"}
        onOpenFile={() => undefined}
      />,
    );

    const figure = await screen.findByTestId("plan-mermaid");
    expect(figure.querySelector("svg")).not.toBeNull();
    expect(renderMock).toHaveBeenCalledTimes(1);
  });
});
