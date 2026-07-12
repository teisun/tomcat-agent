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
  text,
  preview = false,
}: {
  preview?: boolean;
  text: string | undefined;
}) {
  return (
    <pre
      className={`tc-terminal-output${preview ? " tc-terminal-output--preview" : ""}`}
      data-testid={preview ? "terminal-output-preview" : "tool-row-terminal"}
    >
      {normalizeTerminalText(text)}
    </pre>
  );
}
