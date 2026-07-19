import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MarkdownBody } from "./MarkdownBody";

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

describe("MarkdownBody", () => {
  it("renders headings, lists, inline code and bold from markdown", () => {
    render(
      <MarkdownBody
        markdown={"# Title\n\nA **bold** word and `inline`.\n\n- one\n- two"}
        onOpenLink={() => undefined}
      />,
    );
    const body = screen.getByTestId("plan-markdown-body");
    expect(body.querySelector("h1")?.textContent).toContain("Title");
    expect(body.querySelector("strong")?.textContent).toBe("bold");
    expect(body.querySelector("code")?.textContent).toBe("inline");
    expect(body.querySelectorAll("li")).toHaveLength(2);
  });

  it("stamps blocks with absolute data-source-line from the source map (inline markdown included)", () => {
    render(
      <MarkdownBody
        markdown={"# Title\n\nA **bold** word.\n\n- one\n- two"}
        onOpenLink={() => undefined}
        sourceLineMap={[5, 6, 7, 8, 9, 10]}
      />,
    );
    const body = screen.getByTestId("plan-markdown-body");
    expect(body.querySelector("h1")?.getAttribute("data-source-line")).toBe("5");
    const paragraph = body.querySelector("p");
    expect(paragraph?.getAttribute("data-source-line")).toBe("7");
    // The inline **bold** survives and no longer breaks the line mapping.
    expect(paragraph?.querySelector("strong")?.textContent).toBe("bold");
    expect(body.querySelector("ul")?.getAttribute("data-source-line")).toBe("9");
  });

  it("omits data-source-line when no source map is provided", () => {
    render(<MarkdownBody markdown={"# Title\n\ntext"} onOpenLink={() => undefined} />);
    const body = screen.getByTestId("plan-markdown-body");
    expect(body.querySelector("h1")?.hasAttribute("data-source-line")).toBe(false);
    expect(body.querySelector("p")?.hasAttribute("data-source-line")).toBe(false);
  });

  it("renders fenced code as a highlighted code card in the plan preview", () => {
    render(
      <MarkdownBody
        markdown={"```ts src/plan-preview.ts:3\nconst answer = 42;\n```\n"}
        onOpenFile={() => undefined}
        onOpenLink={() => undefined}
      />,
    );

    const card = screen.getByTestId("assistant-code-card");
    expect(card.querySelector(".tc-code-card__header")).not.toBeNull();
    expect(card.querySelector("code.hljs")?.textContent).toBe("const answer = 42;\n");
    expect(card.querySelector("[data-testid='assistant-code-copy']")).not.toBeNull();
  });

  it("intercepts link clicks and forwards them without navigating", () => {
    const onOpenLink = vi.fn();
    render(
      <MarkdownBody markdown={"[docs](https://example.com/docs)"} onOpenLink={onOpenLink} />,
    );
    const anchor = screen.getByTestId("plan-markdown-body").querySelector("a") as HTMLAnchorElement;
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor.dispatchEvent(event);
    expect(onOpenLink).toHaveBeenCalledWith("https://example.com/docs");
    expect(event.defaultPrevented).toBe(true);
  });

  it("keeps ordinary markdown links on the openLink path", () => {
    const onOpenLink = vi.fn();
    const onOpenFile = vi.fn();
    render(
      <MarkdownBody
        markdown={"Read [the design doc](docs/design.md)."}
        onOpenFile={onOpenFile}
        onOpenLink={onOpenLink}
      />,
    );
    const anchor = screen.getByTestId("plan-markdown-body").querySelector("a") as HTMLAnchorElement;
    fireEvent.click(anchor);
    expect(onOpenLink).toHaveBeenCalledWith("docs/design.md");
    expect(onOpenFile).not.toHaveBeenCalled();
  });

  it("linkifies inline file paths and routes clicks to onOpenFile", () => {
    const onOpenFile = vi.fn();
    const onOpenLink = vi.fn();
    render(
      <MarkdownBody
        markdown={"Review `src/test/fixtures/plan-preview.ts:18` before shipping."}
        onOpenFile={onOpenFile}
        onOpenLink={onOpenLink}
      />,
    );
    const link = screen.getByTestId("assistant-clickable-path");
    expect(link.textContent).toContain("plan-preview.ts:18");
    expect(link.textContent).not.toContain("src/test/fixtures/");
    expect(link.getAttribute("title")).toBe("src/test/fixtures/plan-preview.ts:18");
    fireEvent.click(link);
    expect(onOpenFile).toHaveBeenCalledWith("src/test/fixtures/plan-preview.ts", 18);
    expect(onOpenLink).not.toHaveBeenCalled();
  });

  it("strips dangerous script content via DOMPurify", () => {
    render(
      <MarkdownBody
        markdown={"Hello\n\n<script>window.__pwned = true;</script>\n\n<img src=x onerror=\"window.__pwned = true\">"}
        onOpenLink={() => undefined}
      />,
    );
    const body = screen.getByTestId("plan-markdown-body");
    expect(body.querySelector("script")).toBeNull();
    const img = body.querySelector("img");
    expect(img?.getAttribute("onerror")).toBeNull();
    expect((window as unknown as { __pwned?: boolean }).__pwned).toBeUndefined();
  });

  it("renders a mermaid code block into an inline SVG diagram", async () => {
    renderMock.mockClear();
    render(
      <MarkdownBody
        markdown={"# Flow\n\n```mermaid\nflowchart LR\n  a --> b\n```\n"}
        onOpenLink={() => undefined}
      />,
    );

    const figure = await screen.findByTestId("plan-mermaid");
    expect(figure.tagName.toLowerCase()).toBe("figure");
    expect(figure.querySelector("svg")).not.toBeNull();
    expect(renderMock).toHaveBeenCalledTimes(1);
    expect(renderMock.mock.calls[0][1]).toContain("flowchart LR");
    // The original <pre>/<code.language-mermaid> is replaced by the figure.
    expect(
      screen.getByTestId("plan-markdown-body").querySelector("code.language-mermaid"),
    ).toBeNull();
  });

  it("keeps the code block when mermaid rendering fails", async () => {
    renderMock.mockRejectedValueOnce(new Error("bad graph"));
    render(
      <MarkdownBody
        markdown={"```mermaid\nnope\n```\n"}
        onOpenLink={() => undefined}
      />,
    );
    await waitFor(() => {
      const code = screen
        .getByTestId("plan-markdown-body")
        .querySelector("code.language-mermaid");
      expect(code?.closest("pre")?.getAttribute("data-mermaid-error")).toBe("1");
    });
    expect(screen.queryByTestId("plan-mermaid")).toBeNull();
  });
});
