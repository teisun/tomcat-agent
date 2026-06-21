import { useState } from "react";

import type { WebviewThinkingBlock } from "../types";

export function ThinkingBlock({ item }: { item: WebviewThinkingBlock }) {
  const [collapsed, setCollapsed] = useState(true);

  return (
    <section className="tc-thinking" data-testid="thinking-block">
      <button
        aria-label={collapsed ? "Expand thinking" : "Collapse thinking"}
        className="tc-thinking__toggle"
        data-testid="thinking-toggle"
        onClick={() => setCollapsed((value) => !value)}
        type="button"
      >
        <span>Tomcat · Thinking</span>
        <span>{collapsed ? "▸" : "▾"}</span>
      </button>
      {collapsed ? null : <pre>{item.text}</pre>}
    </section>
  );
}
