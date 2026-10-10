import { en, type MessageKey } from "./en";
import { zhCN } from "./zh-CN";

export type { MessageKey, TermKey, ZhCatalog } from "./en";
export type Locale = "en" | "zh-CN";
export type LanguagePreference = Locale | "auto";
export interface UiPreferences { language: LanguagePreference; effective: Locale; envOverride: boolean }
export type MessageArgs = Readonly<Record<string, string | number>>;
export type Translator = (key: MessageKey, args?: MessageArgs) => string;

let currentLocale: Locale = "en";
const listeners = new Set<() => void>();
export const getLocale = (): Locale => currentLocale;
export function setLocale(locale: Locale): void {
  if (locale === currentLocale) return;
  currentLocale = locale;
  for (const listener of listeners) listener();
}
export function subscribeLocale(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
export function normalizeLocale(tag: string): Locale {
  return tag.trim().split(/[-_.@]/u)[0].toLowerCase() === "zh" ? "zh-CN" : "en";
}
export function isLanguagePreference(value: unknown): value is LanguagePreference {
  return value === "auto" || value === "en" || value === "zh-CN";
}
export function isUiPreferences(value: unknown): value is UiPreferences {
  if (!value || typeof value !== "object") return false;
  const v = value as Partial<UiPreferences>;
  return isLanguagePreference(v.language) && (v.effective === "en" || v.effective === "zh-CN") && typeof v.envOverride === "boolean";
}

/** The target catalog only overrides English; term.* deliberately falls back silently. */
export function renderMessage(catalog: Readonly<Partial<Record<MessageKey, string>>>, key: MessageKey, args: MessageArgs = {}): string {
  const template = catalog[key] ?? en[key];
  if (template === undefined) return catalog["error.unknown"] ?? en["error.unknown"];
  let missing = false;
  const message = template.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/gu, (placeholder, name: string) => {
    const value = args[name];
    if (value === undefined) { missing = true; return placeholder; }
    return String(value);
  });
  return missing ? (catalog["error.unknown"] ?? en["error.unknown"]) : message;
}

export function translate(locale: Locale, key: MessageKey, args?: MessageArgs): string {
  return renderMessage(locale === "zh-CN" ? zhCN : en, key, args);
}
export const t: Translator = (key, args) => translate(getLocale(), key, args);
export function pluralKey<K extends MessageKey & `${string}.other`>(locale: Locale, other: K, count: number): MessageKey {
  const one = other.replace(/\.other$/u, ".one") as MessageKey;
  return new Intl.PluralRules(locale).select(count) === "one" && one in en ? one : other;
}
