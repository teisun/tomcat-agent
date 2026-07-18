const KNOWN_FILE_EXTENSIONS = new Set([
  "c",
  "cc",
  "cpp",
  "css",
  "go",
  "gif",
  "h",
  "hpp",
  "html",
  "ico",
  "java",
  "jpeg",
  "jpg",
  "js",
  "json",
  "jsx",
  "kt",
  "md",
  "mjs",
  "php",
  "png",
  "py",
  "rb",
  "rs",
  "scala",
  "sh",
  "sql",
  "svg",
  "swift",
  "toml",
  "ts",
  "tsx",
  "txt",
  "xml",
  "yaml",
  "yml",
]);

export interface InlineFilePathMatch {
  column?: number;
  line?: number;
  originalText: string;
  path: string;
}

function hasUriScheme(value: string): boolean {
  if (/^[a-z]:[\\/]/iu.test(value)) {
    return false;
  }
  return /^(?:[a-z][a-z0-9+.-]*:\/\/|mailto:|command:)/iu.test(value);
}

export function basenameOf(value: string): string {
  const normalized = value.replaceAll("\\", "/");
  const last = normalized.split("/").pop();
  return last && last.length > 0 ? last : normalized;
}

function fileExtension(value: string): string | undefined {
  const base = basenameOf(value);
  const ext = base.split(".").pop()?.toLowerCase();
  return ext && base.includes(".") ? ext : undefined;
}

export function inferLanguageFromPath(filePath: string): string | undefined {
  switch (fileExtension(filePath)) {
    case "bash":
    case "sh":
      return "bash";
    case "c":
    case "h":
      return "c";
    case "cc":
    case "cpp":
    case "hpp":
      return "cpp";
    case "css":
      return "css";
    case "go":
      return "go";
    case "html":
    case "xml":
      return "xml";
    case "java":
      return "java";
    case "jpeg":
    case "jpg":
    case "png":
    case "gif":
    case "svg":
    case "ico":
      return "plaintext";
    case "js":
    case "jsx":
    case "mjs":
      return "javascript";
    case "json":
      return "json";
    case "kt":
      return "kotlin";
    case "md":
      return "markdown";
    case "php":
      return "php";
    case "py":
      return "python";
    case "rb":
      return "ruby";
    case "rs":
      return "rust";
    case "scala":
      return "scala";
    case "sql":
      return "sql";
    case "swift":
      return "swift";
    case "toml":
      return "ini";
    case "ts":
    case "tsx":
      return "typescript";
    case "yaml":
    case "yml":
      return "yaml";
    case "txt":
      return "plaintext";
    default:
      return undefined;
  }
}

export function looksLikeFilePathToken(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || /\s/u.test(trimmed) || hasUriScheme(trimmed)) {
    return false;
  }
  if (
    trimmed.startsWith("./")
    || trimmed.startsWith("../")
    || trimmed.startsWith("~/")
    || trimmed.startsWith("/")
    || /^[a-z]:[\\/]/iu.test(trimmed)
  ) {
    return true;
  }
  if (trimmed.includes("/") || trimmed.includes("\\")) {
    return true;
  }
  const ext = fileExtension(trimmed);
  return typeof ext === "string" && KNOWN_FILE_EXTENSIONS.has(ext);
}

export function splitInlinePathLocation(value: string): InlineFilePathMatch | null {
  const trimmed = value.trim();
  if (!trimmed || hasUriScheme(trimmed)) {
    return null;
  }

  const hashMatch = trimmed.match(/^(.*)#L(\d+)(?:C(\d+))?$/u);
  if (hashMatch) {
    const path = hashMatch[1];
    if (!looksLikeFilePathToken(path)) {
      return null;
    }
    return {
      column: hashMatch[3] ? Number(hashMatch[3]) : undefined,
      line: Number(hashMatch[2]),
      originalText: trimmed,
      path,
    };
  }

  const colonMatch = trimmed.match(/^(.*):(\d+)(?::(\d+))?$/u);
  if (colonMatch) {
    const path = colonMatch[1];
    if (path.length > 1 && looksLikeFilePathToken(path)) {
      return {
        column: colonMatch[3] ? Number(colonMatch[3]) : undefined,
        line: Number(colonMatch[2]),
        originalText: trimmed,
        path,
      };
    }
  }

  if (!looksLikeFilePathToken(trimmed)) {
    return null;
  }
  return {
    originalText: trimmed,
    path: trimmed,
  };
}

export function detectInlineFilePath(value: string): InlineFilePathMatch | null {
  return splitInlinePathLocation(value);
}
