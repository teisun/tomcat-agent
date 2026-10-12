import { inferLanguageFromPath, splitInlinePathLocation, looksLikeFilePathToken } from "./inlinePath";

export interface ParsedCodeFenceInfo {
  filePath?: string;
  isMermaid: boolean;
  language?: string;
  languageLabel: string;
  line?: number;
  rawInfo: string;
}

function normalizeExplicitLanguage(value: string | undefined): string | undefined {
  if (!value) {
    return undefined;
  }
  switch (value.toLowerCase()) {
    case "cjs":
    case "js":
    case "jsx":
    case "mjs":
      return "javascript";
    case "html":
      return "xml";
    case "md":
      return "markdown";
    case "py":
      return "python";
    case "rb":
      return "ruby";
    case "rs":
      return "rust";
    case "sh":
    case "shell":
    case "zsh":
      return "bash";
    case "text":
    case "txt":
      return "plaintext";
    case "ts":
    case "tsx":
      return "typescript";
    case "yml":
      return "yaml";
    default:
      return value.toLowerCase();
  }
}

function presentLanguageLabel(value: string | undefined): string {
  if (!value || value === "plaintext") {
    return "text";
  }
  return value;
}

export function parseCodeFenceInfo(rawInfo: string | null | undefined): ParsedCodeFenceInfo {
  const info = rawInfo?.trim() ?? "";
  if (!info) {
    return {
      isMermaid: false,
      language: "plaintext",
      languageLabel: "text",
      rawInfo: "",
    };
  }

  const tokens = info.split(/\s+/u).filter(Boolean);
  const first = tokens[0];
  const second = tokens[1];

  if (first.toLowerCase() === "mermaid") {
    return {
      isMermaid: true,
      language: "mermaid",
      languageLabel: "mermaid",
      rawInfo: info,
    };
  }

  if (looksLikeFilePathToken(first)) {
    const fileMatch = splitInlinePathLocation(first);
    const language = fileMatch ? inferLanguageFromPath(fileMatch.path) ?? "plaintext" : "plaintext";
    return {
      filePath: fileMatch?.path,
      isMermaid: false,
      language,
      languageLabel: presentLanguageLabel(language),
      line: fileMatch?.line,
      rawInfo: info,
    };
  }

  const explicitLanguage = normalizeExplicitLanguage(first);
  if (second && looksLikeFilePathToken(second)) {
    const fileMatch = splitInlinePathLocation(second);
    return {
      filePath: fileMatch?.path,
      isMermaid: false,
      language: explicitLanguage ?? inferLanguageFromPath(fileMatch?.path ?? "") ?? "plaintext",
      languageLabel: first,
      line: fileMatch?.line,
      rawInfo: info,
    };
  }

  return {
    isMermaid: false,
    language: explicitLanguage ?? "plaintext",
    languageLabel: first,
    rawInfo: info,
  };
}
