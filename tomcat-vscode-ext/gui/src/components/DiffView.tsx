import type { FileDiffLine } from "../types";

type RenderRow =
  | {
      kind: "fold";
      hiddenCount: number;
      key: string;
      label?: string;
    }
  | {
      key: string;
      kind: "line";
      line: FileDiffLine;
    };

const CONTEXT_HEAD = 2;
const CONTEXT_TAIL = 2;
const MIN_CONTEXT_TO_FOLD = 7;
const PREVIEW_LEADING_CONTEXT = 2;

function countRenderedLines(rows: RenderRow[]): number {
  return rows.reduce(
    (total, row) => total + (row.kind === "line" ? 1 : row.hiddenCount),
    0,
  );
}

function collapseContextRows(diff: FileDiffLine[]): RenderRow[] {
  const rows: RenderRow[] = [];
  let index = 0;
  while (index < diff.length) {
    const current = diff[index];
    if (current.tag !== "ctx") {
      rows.push({
        key: `line-${index}-${current.oldLine ?? "n"}-${current.newLine ?? "n"}`,
        kind: "line",
        line: current,
      });
      index += 1;
      continue;
    }

    const runStart = index;
    while (index < diff.length && diff[index].tag === "ctx") {
      index += 1;
    }
    const run = diff.slice(runStart, index);
    if (run.length < MIN_CONTEXT_TO_FOLD) {
      run.forEach((line, runIndex) => {
        rows.push({
          key: `line-${runStart + runIndex}-${line.oldLine ?? "n"}-${line.newLine ?? "n"}`,
          kind: "line",
          line,
        });
      });
      continue;
    }

    run.slice(0, CONTEXT_HEAD).forEach((line, runIndex) => {
      rows.push({
        key: `line-${runStart + runIndex}-${line.oldLine ?? "n"}-${line.newLine ?? "n"}`,
        kind: "line",
        line,
      });
    });
    rows.push({
      hiddenCount: run.length - CONTEXT_HEAD - CONTEXT_TAIL,
      key: `fold-${runStart}`,
      kind: "fold",
    });
    run.slice(-CONTEXT_TAIL).forEach((line, runIndex) => {
      rows.push({
        key: `line-${index - CONTEXT_TAIL + runIndex}-${line.oldLine ?? "n"}-${line.newLine ?? "n"}`,
        kind: "line",
        line,
      });
    });
  }
  return rows;
}

function renderLineNumber(value: number | null | undefined): string {
  return typeof value === "number" ? String(value) : "";
}

function previewRows(rows: RenderRow[], maxRows: number): RenderRow[] {
  if (rows.length <= maxRows) {
    return rows;
  }

  const firstChangeIndex = rows.findIndex(
    (row) => row.kind === "line" && row.line.tag !== "ctx",
  );
  let start =
    firstChangeIndex === -1
      ? 0
      : Math.max(0, firstChangeIndex - PREVIEW_LEADING_CONTEXT);

  const leadingFold =
    start > 0 && rows[start - 1]?.kind === "fold" ? [rows[start - 1]] : [];
  const end = Math.min(rows.length, start + maxRows);
  const preview = [...leadingFold, ...rows.slice(start, end)];
  if (end < rows.length) {
    preview.push({
      hiddenCount: countRenderedLines(rows.slice(end)),
      key: `fold-preview-more-${end}`,
      kind: "fold",
      label: `${countRenderedLines(rows.slice(end))} more lines`,
    });
  }
  return preview;
}

function diffRowClass(tag: FileDiffLine["tag"]): string {
  switch (tag) {
    case "add":
      return "tc-diff-view__row tc-diff-view__row--add";
    case "del":
      return "tc-diff-view__row tc-diff-view__row--del";
    default:
      return "tc-diff-view__row tc-diff-view__row--ctx";
  }
}

export function DiffView({
  diff,
  previewRows: previewLimit,
}: {
  diff?: FileDiffLine[];
  previewRows?: number;
}) {
  if (!diff) {
    return (
      <div className="tc-diff-view__empty" data-testid="diff-view-empty">
        File too large to render inline diff. Showing summary only.
      </div>
    );
  }

  if (diff.length === 0) {
    return (
      <div className="tc-diff-view__empty" data-testid="diff-view-empty">
        No line changes to display.
      </div>
    );
  }

  const fullRows = collapseContextRows(diff);
  const rows = previewLimit ? previewRows(fullRows, previewLimit) : fullRows;

  return (
    <div
      className={`tc-diff-view${previewLimit ? " tc-diff-view--preview" : ""}`}
      data-testid={previewLimit ? "diff-view-preview" : "diff-view"}
    >
      {rows.map((row) =>
        row.kind === "fold" ? (
          <div className="tc-diff-view__fold" data-testid="diff-fold-marker" key={row.key}>
            {row.label ?? `${row.hiddenCount} unmodified lines`}
          </div>
        ) : (
          <div className={diffRowClass(row.line.tag)} key={row.key}>
            <span className="tc-diff-view__gutter tc-diff-view__gutter--old">
              {renderLineNumber(row.line.oldLine)}
            </span>
            <span className="tc-diff-view__gutter tc-diff-view__gutter--new">
              {renderLineNumber(row.line.newLine)}
            </span>
            <span className="tc-diff-view__sign" aria-hidden="true">
              {row.line.tag === "add" ? "+" : row.line.tag === "del" ? "-" : " "}
            </span>
            <span className="tc-diff-view__text">{row.line.text}</span>
          </div>
        ),
      )}
    </div>
  );
}
