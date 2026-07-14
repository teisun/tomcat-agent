import type {
  PlanFileState,
  PlanTodo,
  PlanTodoStatus,
} from "../../shared/planPreviewProtocol";

export const PLAN_TITLE_MAX = 96;

/**
 * Strip a single layer of matching YAML quotes from a scalar value. Kept
 * dependency-free and intentionally forgiving — plan frontmatter is written by
 * `serde_yaml` (block style) but the engineering doc examples are hand-authored.
 */
export function stripYamlQuotes(value: string): string {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

export function truncatePlanTitle(value: string): string {
  const firstLine = value.split("\n")[0]?.trim() ?? "";
  if (firstLine.length <= PLAN_TITLE_MAX) {
    return firstLine;
  }
  return `${firstLine.slice(0, PLAN_TITLE_MAX - 3).trimEnd()}...`;
}

export interface ParsedPlanDocument {
  /**
   * 1-based source line in the original file for each line of `bodyMarkdown`
   * (`bodyLineMap[i]` ↔ `bodyMarkdown.split("\n")[i]`). Lets the preview map a
   * rendered selection back to an absolute file line even though the body is
   * offset by the frontmatter and may have the Todos Board spliced out.
   */
  bodyLineMap: number[];
  bodyMarkdown: string;
  overview: string | null;
  planId: string | null;
  raw: string;
  state: PlanFileState | null;
  title: string | null;
  todos: PlanTodo[];
}

/**
 * Map each line of the (already frontmatter-stripped, board-stripped) body back
 * to its 1-based line in the original file. `bodyMarkdown` lines are a forward
 * subsequence of the raw lines (blank lines and the Todos Board are dropped, the
 * last line may be right-trimmed, and a single blank spacer may be inserted), so
 * a monotonic forward scan recovers the mapping. Unmatched lines (spacer /
 * trimmed) reuse the previous mapping to stay monotonic.
 */
export function mapBodyLinesToRaw(
  rawLines: string[],
  bodyMarkdown: string,
  bodyStartRawLine: number,
): number[] {
  const bodyLines = bodyMarkdown.split("\n");
  const map: number[] = new Array(bodyLines.length);
  let rawCursor = Math.max(0, bodyStartRawLine - 1);
  let lastMapped = bodyStartRawLine;
  for (let b = 0; b < bodyLines.length; b += 1) {
    const needle = bodyLines[b];
    let found = -1;
    for (let r = rawCursor; r < rawLines.length; r += 1) {
      const rawLine = rawLines[r];
      if (rawLine === needle || rawLine.replace(/[ \t]+$/u, "") === needle) {
        found = r;
        break;
      }
    }
    if (found === -1) {
      map[b] = lastMapped;
      continue;
    }
    map[b] = found + 1;
    lastMapped = found + 1;
    rawCursor = found + 1;
  }
  return map;
}

const TODO_BOARD_BEGIN = "<!-- todos-board:auto:begin -->";
const TODO_BOARD_END = "<!-- todos-board:auto:end -->";

function normalizeTodoStatus(value: string): PlanTodoStatus {
  switch (value.trim()) {
    case "in_progress":
      return "in_progress";
    case "completed":
      return "completed";
    case "cancelled":
      return "cancelled";
    default:
      return "pending";
  }
}

function normalizePlanState(value: string): PlanFileState | null {
  switch (value.trim().toLowerCase()) {
    case "planning":
      return "planning";
    case "executing":
      return "executing";
    case "completed":
      return "completed";
    case "pending":
      return "pending";
    default:
      return null;
  }
}

function assignTodoField(todo: Partial<PlanTodo>, key: string, rawValue: string): void {
  const value = stripYamlQuotes(rawValue);
  if (key === "id") {
    todo.id = value;
  } else if (key === "content") {
    todo.content = value;
  } else if (key === "status") {
    todo.status = normalizeTodoStatus(value);
  }
}

/**
 * Remove the auto-maintained `## Todos Board` section (heading + marker block)
 * so the rendered body never duplicates the four-state checklist shown below.
 * The `<!-- todos-board:auto:* -->` markers wrap only the content; the heading
 * lives just above the begin marker, so we walk backwards to include it.
 */
function stripTodosBoard(body: string): string {
  if (!body.includes(TODO_BOARD_BEGIN) || !body.includes(TODO_BOARD_END)) {
    return body;
  }
  const lines = body.split("\n");
  let beginLine = -1;
  let endLine = -1;
  for (let i = 0; i < lines.length; i += 1) {
    if (beginLine === -1 && lines[i].includes(TODO_BOARD_BEGIN)) {
      beginLine = i;
    }
    if (beginLine !== -1 && lines[i].includes(TODO_BOARD_END)) {
      endLine = i;
      break;
    }
  }
  if (beginLine === -1 || endLine === -1 || endLine < beginLine) {
    return body;
  }
  let start = beginLine;
  let cursor = beginLine - 1;
  while (cursor >= 0 && lines[cursor].trim() === "") {
    cursor -= 1;
  }
  if (cursor >= 0 && /^#{1,6}[ \t]+Todos Board[ \t]*$/.test(lines[cursor].trim())) {
    start = cursor;
  }
  const head = lines.slice(0, start);
  const tail = lines.slice(endLine + 1);
  while (head.length > 0 && head[head.length - 1].trim() === "") {
    head.pop();
  }
  while (tail.length > 0 && tail[0].trim() === "") {
    tail.shift();
  }
  const spacer = head.length > 0 && tail.length > 0 ? [""] : [];
  return [...head, ...spacer, ...tail].join("\n");
}

function isListItem(line: string): boolean {
  return /^\s*-\s*/.test(line);
}

function matchTopLevelKey(line: string): { key: string; value: string } | null {
  const match = line.match(/^([A-Za-z][\w-]*):\s*(.*)$/);
  if (!match) {
    return null;
  }
  return { key: match[1], value: match[2] };
}

/**
 * Parse a `.plan.md` document into the fields the preview needs. Zero runtime
 * dependencies — this is the single source of truth for plan frontmatter,
 * shared by the preview editor and the chat webview's PlanFileCard.
 */
export function parsePlanDocument(text: string): ParsedPlanDocument {
  const normalized = text.replace(/\r\n/g, "\n");
  const empty: ParsedPlanDocument = {
    bodyLineMap: [],
    bodyMarkdown: "",
    overview: null,
    planId: null,
    raw: text,
    state: null,
    title: null,
    todos: [],
  };

  const lines = normalized.split("\n");
  if (lines[0]?.trim() !== "---") {
    const bodyMarkdown = stripTodosBoard(normalized).replace(/^\n+/, "").replace(/[ \t\n]+$/, "");
    return {
      ...empty,
      bodyLineMap: mapBodyLinesToRaw(lines, bodyMarkdown, 1),
      bodyMarkdown,
    };
  }

  let fmEnd = -1;
  for (let i = 1; i < lines.length; i += 1) {
    if (lines[i].trim() === "---") {
      fmEnd = i;
      break;
    }
  }
  if (fmEnd === -1) {
    const bodyMarkdown = stripTodosBoard(normalized).replace(/^\n+/, "").replace(/[ \t\n]+$/, "");
    return {
      ...empty,
      bodyLineMap: mapBodyLinesToRaw(lines, bodyMarkdown, 1),
      bodyMarkdown,
    };
  }

  const fmLines = lines.slice(1, fmEnd);
  const body = lines.slice(fmEnd + 1).join("\n");

  let title: string | null = null;
  let goalValue: string | null = null;
  let overview: string | null = null;
  let planId: string | null = null;
  let state: PlanFileState | null = null;
  const todos: PlanTodo[] = [];

  let i = 0;
  while (i < fmLines.length) {
    const line = fmLines[i];
    const top = matchTopLevelKey(line);
    if (top && top.key === "todos" && !isListItem(line)) {
      i += 1;
      let current: Partial<PlanTodo> | null = null;
      const flush = () => {
        if (current && typeof current.id === "string" && current.id.length > 0) {
          todos.push({
            content: current.content ?? "",
            id: current.id,
            status: current.status ?? "pending",
          });
        }
        current = null;
      };
      while (i < fmLines.length) {
        const inner = fmLines[i];
        if (inner.trim() === "") {
          i += 1;
          continue;
        }
        const innerTop = matchTopLevelKey(inner);
        const listItem = isListItem(inner);
        if (
          innerTop &&
          !listItem &&
          innerTop.key !== "id" &&
          innerTop.key !== "content" &&
          innerTop.key !== "status"
        ) {
          break;
        }
        if (listItem) {
          flush();
          current = {};
          const afterDash = inner.replace(/^\s*-\s*/, "");
          const kv = matchTopLevelKey(afterDash);
          if (kv) {
            assignTodoField(current, kv.key, kv.value);
          }
          i += 1;
          continue;
        }
        const kv = matchTopLevelKey(inner.trim());
        if (kv && current) {
          assignTodoField(current, kv.key, kv.value);
        }
        i += 1;
      }
      flush();
      continue;
    }
    if (top) {
      const value = stripYamlQuotes(top.value);
      if (value) {
        if (top.key === "title" || top.key === "name") {
          title = value;
        } else if (top.key === "goal") {
          goalValue = value;
        } else if (top.key === "overview") {
          overview = value;
        } else if (top.key === "plan_id" || top.key === "planId") {
          planId = value;
        } else if (top.key === "state") {
          state = normalizePlanState(value);
        }
      }
    }
    i += 1;
  }

  if (!title && goalValue) {
    title = truncatePlanTitle(goalValue);
  }

  const bodyMarkdown = stripTodosBoard(body).replace(/^\n+/, "").replace(/[ \t\n]+$/, "");
  return {
    bodyLineMap: mapBodyLinesToRaw(lines, bodyMarkdown, fmEnd + 2),
    bodyMarkdown,
    overview,
    planId,
    raw: text,
    state,
    title,
    todos,
  };
}
