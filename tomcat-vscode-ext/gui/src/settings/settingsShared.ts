import type { MessageKey, Translator } from "../../../src/shared/i18n";
import type { SettingsIntent, VsCodeApiLike } from "../../../src/shared/settingsProtocol";

export const API_OPTIONS = [
  { labelKey: "term.api.openai", value: "openai" },
  { labelKey: "term.api.responses", value: "openai-responses" },
  { labelKey: "term.api.anthropic", value: "anthropic-messages" },
] as const;

/** Candidates for an address without a fixed protocol; the first is its primary protocol. */
export const ALL_PROTOCOLS: readonly string[] = API_OPTIONS.map((option) => option.value);

export const THINKING_FORMAT_OPTIONS = [
  { labelKey: "term.thinking.openai", value: "openai" },
  { labelKey: "term.thinking.deepseek", value: "deepseek" },
  { labelKey: "term.thinking.zai", value: "zai" },
  { labelKey: "term.thinking.doubao", value: "doubao" },
  { labelKey: "term.thinking.anthropic", value: "anthropic" },
  { labelKey: "term.thinking.adaptive", value: "anthropic-adaptive" },
] as const;

export function formatApiLabel(api: string, t: Translator): string {
  const entry = API_OPTIONS.find((option) => option.value === api);
  return entry ? t(entry.labelKey) : api;
}

export function formatThinkingLabel(thinkingFormat: string | null | undefined, t: Translator): string {
  const entry = THINKING_FORMAT_OPTIONS.find((option) => option.value === thinkingFormat);
  return entry ? t(entry.labelKey) : thinkingFormat ?? "";
}

export function fieldText(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim() : "";
}

export function defaultThinkingFormatForApi(api: string): string {
  switch (fieldText(api)) {
    case "anthropic":
    case "anthropic-messages":
      return "anthropic-adaptive";
    case "deepseek":
      return "deepseek";
    case "zai":
      return "zai";
    case "qwen":
      return "qwen";
    case "doubao":
    case "moonshot":
      return "doubao";
    default:
      return "openai";
  }
}

function hasScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value);
}

export function normalizeBaseUrlInput(value: string | null | undefined): string | null {
  const trimmed = fieldText(value);
  if (!trimmed) {
    return null;
  }
  return hasScheme(trimmed) ? trimmed : `https://${trimmed}`;
}

/** HTTP(S) with a host and no embedded credentials; localhost and LAN addresses stay valid. */
export function baseUrlProblem(value: string | null | undefined): MessageKey | null {
  const normalized = normalizeBaseUrlInput(value);
  if (!normalized) return "models.validate.baseRequired";
  try {
    const parsed = new URL(normalized);
    if ((parsed.protocol !== "http:" && parsed.protocol !== "https:") || !parsed.hostname) {
      return "models.validate.baseInvalid";
    }
    if (parsed.username || parsed.password) return "models.validate.baseCredentials";
    return null;
  } catch {
    return "models.validate.baseInvalid";
  }
}

type SettingsIntentPayload = SettingsIntent extends infer Intent
  ? Intent extends SettingsIntent
    ? Omit<Intent, "messageId">
    : never
  : never;

export function send(
  vscodeApi: VsCodeApiLike<SettingsIntent>,
  message: SettingsIntentPayload,
): string {
  const messageId = `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  vscodeApi.postMessage({ ...message, messageId } as SettingsIntent);
  return messageId;
}
