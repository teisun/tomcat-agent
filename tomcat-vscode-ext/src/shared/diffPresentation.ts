import type { FileDiffLine } from "../serveClient/wire";

export interface DiffRange {
  start: number;
  end: number;
}

export interface DiffFragment {
  before: string;
  after: string;
  oldRange?: DiffRange;
  newRange?: DiffRange;
}

export interface DiffPresentation {
  kind: "full" | "fragments";
  fragments: DiffFragment[];
}

export function isDiffViewable<T extends {
  diff?: FileDiffLine[] | null;
  diffTruncated?: boolean;
  diffExpired?: boolean;
}>(tool: T): tool is T & { diff: FileDiffLine[] } {
  return !tool.diffExpired && !tool.diffTruncated &&
    Boolean(tool.diff?.some((line) => line.tag === "add" || line.tag === "del"));
}

function sourceRange(
  lines: readonly FileDiffLine[],
  side: "oldLine" | "newLine",
): DiffRange | undefined {
  // Wire input allows missing line numbers; only claim a range when every line is numbered.
  if (!lines.length || lines.some((line) => typeof line[side] !== "number")) {
    return undefined;
  }
  return { start: lines[0][side]!, end: lines[lines.length - 1][side]! };
}

function renderLines(lines: readonly FileDiffLine[]): string {
  // Terminate each saved logical line so one empty line differs from no lines.
  // The diff does not retain the original EOF newline, so do not infer it here.
  return lines.map((line) => `${line.text}\n`).join("");
}

/** Project saved code only. Each gap separates independently comparable fragments. */
export function createDiffPresentation(
  diff: readonly FileDiffLine[],
): DiffPresentation {
  const presentation: DiffPresentation = { kind: "full", fragments: [] };
  let group: FileDiffLine[] = [];
  const flush = () => {
    if (group.some((line) => line.tag === "add" || line.tag === "del")) {
      const before = group.filter((line) => line.tag !== "add");
      const after = group.filter((line) => line.tag !== "del");
      presentation.fragments.push({
        before: renderLines(before),
        after: renderLines(after),
        oldRange: sourceRange(before, "oldLine"),
        newRange: sourceRange(after, "newLine"),
      });
    }
    group = [];
  };
  for (const line of diff) {
    if (line.tag === "gap") {
      presentation.kind = "fragments";
      flush();
    } else {
      group.push(line);
    }
  }
  flush();
  return presentation;
}
