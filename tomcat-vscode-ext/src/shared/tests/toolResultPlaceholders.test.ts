import fs from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  INTERRUPTED_TOOL_RESULT_TEXT,
  PENDING_TOOL_RESULT_TEXT,
  UNKNOWN_RESTART_TOOL_RESULT_TEXT,
} from "../toolResultPlaceholders";

describe("tool-result placeholder protocol", () => {
  it("matches the exported Rust transcript constants", () => {
    const rustContext = fs.readFileSync(
      path.resolve(process.cwd(), "../tomcat/src/core/session/manager/context.rs"),
      "utf8",
    );

    for (const [name, value] of [
      ["INTERRUPTED_TOOL_RESULT_TEXT", INTERRUPTED_TOOL_RESULT_TEXT],
      ["PENDING_TOOL_RESULT_TEXT", PENDING_TOOL_RESULT_TEXT],
      ["UNKNOWN_RESTART_TOOL_RESULT_TEXT", UNKNOWN_RESTART_TOOL_RESULT_TEXT],
    ] as const) {
      const escaped = value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      expect(rustContext).toMatch(
        new RegExp(`pub const ${name}: &str\\s*=\\s*"${escaped}";`),
      );
    }
  });

  it.each([
    INTERRUPTED_TOOL_RESULT_TEXT,
    PENDING_TOOL_RESULT_TEXT,
    UNKNOWN_RESTART_TOOL_RESULT_TEXT,
  ])("uses a system-narration marker: %s", (value) => {
    expect(value).toMatch(/^\[.+]$/);
  });

  it("keeps all protocol markers distinct", () => {
    expect(new Set([
      INTERRUPTED_TOOL_RESULT_TEXT,
      PENDING_TOOL_RESULT_TEXT,
      UNKNOWN_RESTART_TOOL_RESULT_TEXT,
    ]).size).toBe(3);
  });
});
