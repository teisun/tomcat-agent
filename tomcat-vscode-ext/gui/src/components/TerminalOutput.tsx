function normalizeTerminalText(text: string | undefined): string {
  return (text ?? "").replace(/\r\n/g, "\n");
}

export function takeUnicodeSafeSuffix(
  text: string,
  maxCodeUnits: number,
): string {
  if (text.length <= maxCodeUnits) return text;
  let start = text.length - maxCodeUnits;
  const current = text.charCodeAt(start);
  const previous = text.charCodeAt(start - 1);
  if (
    current >= 0xdc00 &&
    current <= 0xdfff &&
    previous >= 0xd800 &&
    previous <= 0xdbff
  ) {
    start += 1;
  }
  return text.slice(start);
}

export function tailTerminalOutput(
  text: string | undefined,
  lines: number,
): string {
  const normalized = normalizeTerminalText(text);
  if (!normalized) return "";
  const hasTrailingNewline = normalized.endsWith("\n");
  const parts = (
    hasTrailingNewline ? normalized.slice(0, -1) : normalized
  ).split("\n");
  const tail = parts.slice(-lines).join("\n");
  return hasTrailingNewline ? `${tail}\n` : tail;
}

export function limitTerminalOutput(text: string | undefined): string {
  let output = normalizeTerminalText(text);
  if (output.length > 30_000) output = takeUnicodeSafeSuffix(output, 30_000);
  const lines = output.split("\n");
  return lines.length > 500 ? lines.slice(-500).join("\n") : output;
}

export function TerminalOutput({
  command,
  text,
  preview = false,
}: {
  /** Optional shell command rendered as a `$ …` prompt line above the output. */
  command?: string;
  preview?: boolean;
  text: string | undefined;
}) {
  const output = normalizeTerminalText(text);
  const commandLine = command?.trim();
  return (
    <pre
      className={`tc-terminal-output${preview ? " tc-terminal-output--preview" : ""}`}
      data-testid={preview ? "terminal-output-preview" : "tool-row-terminal"}
    >
      {commandLine ? (
        <span
          className="tc-terminal-output__cmd"
          data-testid="terminal-output-cmd"
        >
          <span aria-hidden="true" className="tc-terminal-output__prompt">
            ${" "}
          </span>
          {commandLine}
          {output ? "\n" : ""}
        </span>
      ) : null}
      {output}
    </pre>
  );
}
