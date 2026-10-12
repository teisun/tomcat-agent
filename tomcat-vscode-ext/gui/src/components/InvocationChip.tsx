import { useT } from "../i18n/LocaleProvider";
import type { WebviewInstruction } from "../types";

export function InvocationChip({ instruction, selected = false }: { instruction: WebviewInstruction; selected?: boolean }) {
  const t = useT();
  const kind = t(instruction.kind === "skill" ? "term.skill" : "instruction.command");
  return <span className={`tc-chip--invocation${selected ? " tc-chip--invocation-selected" : ""}`} data-testid="invocation-chip" title={`${kind} · ${instruction.path ?? instruction.resourceId}`} aria-label={`${kind} ${instruction.label}`}>{instruction.label}</span>;
}
