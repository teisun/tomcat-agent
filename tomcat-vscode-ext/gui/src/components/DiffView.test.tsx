import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { FileDiffLine } from "../types";
import { DiffView } from "./DiffView";

function buildDiff(lines: FileDiffLine[]): FileDiffLine[] {
  return lines;
}

describe("DiffView", () => {
  it("renders add/delete rows with line numbers", () => {
    render(
      <DiffView
        diff={buildDiff([
          { newLine: 1, oldLine: 1, tag: "ctx", text: "const a = 1;" },
          { newLine: null, oldLine: 2, tag: "del", text: "const b = 2;" },
          { newLine: 2, oldLine: null, tag: "add", text: "const b = 3;" },
        ])}
      />,
    );

    expect(screen.getByTestId("diff-view").textContent).toContain("const b = 2;");
    expect(screen.getByTestId("diff-view").textContent).toContain("const b = 3;");
  });

  it("folds long unchanged context runs in full mode", () => {
    render(
      <DiffView
        diff={buildDiff([
          { newLine: 1, oldLine: 1, tag: "ctx", text: "line 1" },
          { newLine: 2, oldLine: 2, tag: "ctx", text: "line 2" },
          { newLine: 3, oldLine: 3, tag: "ctx", text: "line 3" },
          { newLine: 4, oldLine: 4, tag: "ctx", text: "line 4" },
          { newLine: 5, oldLine: 5, tag: "ctx", text: "line 5" },
          { newLine: 6, oldLine: 6, tag: "ctx", text: "line 6" },
          { newLine: 7, oldLine: 7, tag: "ctx", text: "line 7" },
          { newLine: 8, oldLine: 8, tag: "ctx", text: "line 8" },
          { newLine: 9, oldLine: 9, tag: "add", text: "line 9" },
        ])}
      />,
    );

    expect(screen.getByTestId("diff-fold-marker").textContent).toContain("4 unmodified lines");
    expect(screen.getByTestId("diff-view").textContent).toContain("line 1");
    expect(screen.getByTestId("diff-view").textContent).toContain("line 8");
  });

  it("anchors preview mode around the first change instead of the file tail", () => {
    render(
      <DiffView
        diff={buildDiff([
          { newLine: 1, oldLine: 1, tag: "ctx", text: "line 1" },
          { newLine: 2, oldLine: 2, tag: "ctx", text: "line 2" },
          { newLine: 3, oldLine: 3, tag: "ctx", text: "line 3" },
          { newLine: 4, oldLine: 4, tag: "ctx", text: "line 4" },
          { newLine: 5, oldLine: 5, tag: "ctx", text: "line 5" },
          { newLine: 6, oldLine: 6, tag: "ctx", text: "line 6" },
          { newLine: 7, oldLine: 7, tag: "ctx", text: "line 7" },
          { newLine: 8, oldLine: 8, tag: "ctx", text: "line 8" },
          { newLine: 9, oldLine: 9, tag: "ctx", text: "line 9" },
          { newLine: 10, oldLine: 10, tag: "ctx", text: "line 10" },
          { newLine: null, oldLine: 11, tag: "del", text: "line 11 old" },
          { newLine: 11, oldLine: null, tag: "add", text: "line 11 new" },
          { newLine: 12, oldLine: 12, tag: "ctx", text: "line 12" },
          { newLine: 13, oldLine: 13, tag: "ctx", text: "line 13" },
          { newLine: 14, oldLine: 14, tag: "ctx", text: "line 14" },
          { newLine: 15, oldLine: 15, tag: "ctx", text: "line 15" },
          { newLine: 16, oldLine: 16, tag: "ctx", text: "line 16" },
          { newLine: 17, oldLine: 17, tag: "ctx", text: "line 17" },
          { newLine: 18, oldLine: 18, tag: "ctx", text: "line 18" },
        ])}
        previewRows={5}
      />,
    );

    const preview = screen.getByTestId("diff-view-preview").textContent ?? "";
    expect(preview).not.toContain("line 3");
    expect(preview).toContain("line 9");
    expect(preview).toContain("line 10");
    expect(preview).toContain("line 11 old");
    expect(preview).toContain("line 11 new");
    expect(preview).toContain("line 12");
    expect(preview).not.toContain("line 18");
    expect(preview).toContain("more lines");
  });

  it("shows a fallback message when inline diff is unavailable", () => {
    render(<DiffView diff={undefined} />);

    expect(screen.getByTestId("diff-view-empty").textContent).toContain("File too large");
  });
});
