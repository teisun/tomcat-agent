import { render, screen, waitFor } from "@testing-library/react";
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
