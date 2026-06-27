export function fileChipIconClass(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const base = normalized.split("/").pop() ?? path;
  if (base.endsWith("/") || !base.includes(".")) {
    return "codicon-folder";
  }
  const ext = base.split(".").pop()?.toLowerCase() ?? "";
  switch (ext) {
    case "rs":
      return "codicon-file-code";
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
      return "codicon-file-code";
    case "md":
    case "markdown":
      return "codicon-book";
    case "json":
    case "yaml":
    case "yml":
    case "toml":
      return "codicon-settings-gear";
    default:
      return "codicon-file";
  }
}

function basename(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.split("/").pop() || path;
}

export function FileChip({
  path,
  onOpenFile,
}: {
  path: string;
  onOpenFile(path: string): void;
}) {
  return (
    <button
      className="tc-file-chip"
      data-testid="file-chip"
      onClick={() => onOpenFile(path)}
      type="button"
    >
      <span
        aria-hidden="true"
        className={`tc-file-chip__icon codicon ${fileChipIconClass(path)}`}
        data-testid="file-chip-icon"
      />
      <span className="tc-file-chip__label" data-testid="file-chip-label">
        {basename(path)}
      </span>
    </button>
  );
}
