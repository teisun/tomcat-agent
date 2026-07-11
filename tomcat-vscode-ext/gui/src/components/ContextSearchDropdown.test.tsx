import { act, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";

import {
  ContextSearchDropdown,
  type ContextSearchDropdownHandle,
} from "./ContextSearchDropdown";

const MATCHES = [
  {
    description: "src",
    reference: {
      kind: "file" as const,
      label: "app.ts",
      path: "src/app.ts",
      type: "reference" as const,
    },
  },
  {
    description: "src/components",
    reference: {
      kind: "file" as const,
      label: "AppShell.tsx",
      path: "src/components/AppShell.tsx",
      type: "reference" as const,
    },
  },
];

function renderDropdown({
  loading = false,
  matches = MATCHES,
  onSelect = vi.fn(),
  open = true,
  query = "app",
  truncated = false,
}: {
  loading?: boolean;
  matches?: typeof MATCHES;
  onSelect?: (match: (typeof MATCHES)[number]) => void;
  open?: boolean;
  query?: string;
  truncated?: boolean;
} = {}) {
  const ref = createRef<ContextSearchDropdownHandle>();
  render(
    <ContextSearchDropdown
      ref={ref}
      loading={loading}
      matches={matches}
      onSelect={onSelect}
      open={open}
      query={query}
      truncated={truncated}
    />,
  );
  return { onSelect, ref };
}

describe("ContextSearchDropdown", () => {
  it("renders loading, empty, and truncated states", () => {
    const { rerender } = render(
      <ContextSearchDropdown
        loading
        matches={[]}
        onSelect={() => undefined}
        open
        query="app"
        truncated={false}
      />,
    );

    expect(screen.getByTestId("context-search-loading").textContent).toContain("搜索中");

    rerender(
      <ContextSearchDropdown
        loading={false}
        matches={[]}
        onSelect={() => undefined}
        open
        query="app"
        truncated={false}
      />,
    );
    expect(screen.getByTestId("context-search-empty").textContent).toContain("未找到匹配文件");

    rerender(
      <ContextSearchDropdown
        loading={false}
        matches={MATCHES}
        onSelect={() => undefined}
        open
        query="app"
        truncated
      />,
    );
    expect(screen.getByTestId("context-search-truncated").textContent).toContain(
      "仅显示前 2 条，输入更精确关键词",
    );
  });

  it("keeps the previous list visible while a refined query is still loading", () => {
    renderDropdown({
      loading: true,
      query: "aps",
    });

    expect(screen.queryByTestId("context-search-loading")).toBeNull();
    expect(screen.getAllByTestId("context-search-option")).toHaveLength(2);
    expect(screen.getByTestId("context-search-loading-inline").textContent).toContain("搜索中");
  });

  it("supports keyboard navigation and selection", () => {
    const { onSelect, ref } = renderDropdown();

    const options = screen.getAllByTestId("context-search-option");
    expect(options[0].getAttribute("aria-selected")).toBe("true");
    expect(options[0].getAttribute("role")).toBe("option");
    expect(screen.getByTestId("context-search-dropdown").getAttribute("aria-activedescendant")).toBe(
      options[0].id,
    );

    act(() => {
      ref.current?.onKeyDown(new KeyboardEvent("keydown", { key: "ArrowDown" }));
    });

    const updatedOptions = screen.getAllByTestId("context-search-option");
    expect(updatedOptions[1].getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("context-search-dropdown").getAttribute("aria-activedescendant")).toBe(
      updatedOptions[1].id,
    );

    act(() => {
      ref.current?.onKeyDown(new KeyboardEvent("keydown", { key: "Enter" }));
    });

    expect(onSelect).toHaveBeenCalledWith(MATCHES[1]);
  });

  it("consumes Tab and Escape without adding a legacy '>' marker", () => {
    const { onSelect, ref } = renderDropdown();

    const tabHandled = ref.current?.onKeyDown(new KeyboardEvent("keydown", { key: "Tab" }));
    const escapeHandled = ref.current?.onKeyDown(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(tabHandled).toBe(true);
    expect(escapeHandled).toBe(true);
    expect(onSelect).toHaveBeenCalledWith(MATCHES[0]);
    expect(screen.getAllByTestId("context-search-option")[0].textContent?.trim().startsWith(">")).toBe(
      false,
    );
  });

  it("selects by click and highlights subsequence matches within the basename", () => {
    const { onSelect } = renderDropdown({ query: "ash" });

    const highlights = Array.from(
      document.querySelectorAll(".tc-context-search-dropdown__highlight"),
    ).map((node) => node.textContent);
    expect(highlights).toEqual(["A", "Sh"]);

    fireEvent.click(screen.getAllByTestId("context-search-option")[1]);
    expect(onSelect).toHaveBeenCalledWith(MATCHES[1]);
  });
});
