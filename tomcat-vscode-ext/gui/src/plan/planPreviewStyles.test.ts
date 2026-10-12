import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

const css = readFileSync(path.resolve(process.cwd(), "src/styles.css"), "utf8");

function rule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const match = css.match(new RegExp(`${escaped}\\s*\\{([^}]+)\\}`, "u"));
  expect(match, `missing CSS rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? "";
}

describe("Plan preview reading CSS contract", () => {
  it("defines the VS Code base + 1px token once and applies it to body, code and todos", () => {
    expect(css.match(/--tc-plan-reading-font-size\s*:/gu)).toHaveLength(1);
    expect(rule(".tc-plan-preview")).toContain(
      "--tc-plan-reading-font-size: calc(var(--vscode-font-size, 13px) + 1px)",
    );
    expect(rule(".tc-plan-preview__body")).toContain(
      "font-size: var(--tc-plan-reading-font-size)",
    );
    expect(rule(".tc-plan-preview__body code")).toContain("font-size: 1em");
    expect(rule(".tc-plan-preview__todos-count")).toContain(
      "font-size: var(--tc-plan-reading-font-size)",
    );
    expect(rule(".tc-plan-todos")).toContain("font-size: var(--tc-plan-reading-font-size)");
  });

  it("keeps long content usable at narrow widths and preserves forced-color distinctions", () => {
    expect(rule(".tc-plan-preview__content")).toContain("overflow-x: hidden");
    expect(rule(".tc-plan-todo__content")).toContain("overflow-wrap: anywhere");
    expect(css).toContain("@media (max-width: 480px)");
    expect(css).toContain("@media (forced-colors: active)");
    expect(css).toContain("stroke: CanvasText");
    expect(css).toContain("fill: Highlight");
  });
  it("defines a theme-aware fixed Find widget and inline normal/current highlights", () => {
    expect(rule(".tc-plan-find")).toContain("position: fixed");
    expect(rule(".tc-plan-preview--find-open .tc-plan-preview__content")).toContain(
      "padding-top: 48px",
    );

    expect(rule(".tc-plan-find")).toContain("var(--vscode-editorWidget-background");
    expect(rule(".tc-plan-find__input")).toContain("var(--vscode-input-background)");
    expect(rule(".tc-plan-find__count--empty")).toContain(
      "var(--vscode-inputValidation-errorForeground",
    );

    const normalMatch = rule(".tc-plan-find-fallback-highlight");
    const activeMatch = rule(".tc-plan-find-fallback-highlight--active");
    expect(normalMatch).toContain("rgba(215, 186, 75, 0.28)");
    expect(activeMatch).toContain("rgba(230, 187, 50, 0.46)");
    expect(normalMatch).not.toContain("text-decoration");
    expect(activeMatch).not.toContain("text-decoration");
    expect(css).not.toContain("::highlight(tc-plan-find)");
  });

});
