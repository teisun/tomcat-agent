import type { Editor, Range } from "@tiptap/core";

/** One boundary rule for both / commands and @ references. Never treat a chip as whitespace. */
export function isTriggerBoundary(editor: Pick<Editor, "state">, range: Range): boolean {
  const position = editor.state.doc.resolve(range.from);
  if (position.parentOffset === 0) return true;
  const previous = position.nodeBefore;
  if (!previous) return true;
  if (previous.type.name === "hardBreak") return true;
  return previous.isText && /\s$/u.test(previous.text ?? "");
}
