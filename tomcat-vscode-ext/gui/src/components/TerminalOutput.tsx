function normalizeTerminalText(text: string | undefined): string {
  return (text ?? "").replace(/\r\n/g, "\n");
}

export function tailTerminalOutput(text: string | undefined, lines: number): string {
  const normalized = normalizeTerminalText(text);
  if (!normalized) {
    return "";
  }
  const parts = normalized.split("\n");
  return parts.slice(-lines).join("\n");
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
        <span className="tc-terminal-output__cmd" data-testid="terminal-output-cmd">
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
