import type { TranscriptEntry } from "@paperclipai/adapter-utils";
import { parseAcpxStdoutLine } from "@paperclipai/adapter-utils/acpx-engine/ui";

export function parseTomcatStdoutLine(line: string, ts: string): TranscriptEntry[] {
  return parseAcpxStdoutLine(line, ts).map(entry => {
    // The shared engine's acpx.result frame carries a stop reason, not a bill.
    // Its generic result renderer fills absent usage/cost with zero. Keep the
    // terminal text, but leave accounting to the actual partial usage receipts.
    return entry.kind === "result" ? { kind: "system", ts: entry.ts, text: entry.text } : entry;
  });
}
