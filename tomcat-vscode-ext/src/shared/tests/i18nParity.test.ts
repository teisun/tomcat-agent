import { describe, expect, it } from "vitest";
import { en, type MessageKey, type ZhCatalog } from "../i18n/en";
import { zhCN } from "../i18n/zh-CN";
import { getLocale, normalizeLocale, pluralKey, renderMessage, translate } from "../i18n";

const args = (text: string) => [...text.matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/gu)].map((m) => m[1]).sort();

describe("i18n catalog contracts", () => {
  it("requires every ordinary key while allowing missing terms", () => {
    const valid: ZhCatalog = zhCN;
    void valid;
    const { "session.pin": omitted, ...missing } = zhCN;
    void omitted;
    // @ts-expect-error Ordinary keys must remain required, unlike term.*.
    const invalid: ZhCatalog = missing;
    void invalid;
    for (const [key, value] of Object.entries(zhCN)) {
      expect(key in en).toBe(true);
      expect(args(value)).toEqual(args(en[key as MessageKey]));
      expect(value.trim(), key).not.toBe("");
    }
    for (const key of Object.keys(en)) {
      expect(en[key as MessageKey].trim(), key).not.toBe("");
      if (!key.startsWith("term.")) expect(key in zhCN).toBe(true);
      if (key.endsWith(".one")) expect(key.replace(/\.one$/u, ".other") in zhCN).toBe(true);
    }
  });
  it("falls back for terms, and a supplied translation overrides English", () => {
    const key = "term.mode.plan";
    expect(renderMessage({}, key)).toBe(en[key]);
    const override = `override: ${en[key]}`;
    expect(renderMessage({ [key]: override }, key)).toBe(override);
    expect(translate("en", "not.a.key" as MessageKey)).toBe(en["error.unknown"]);
  });
  it("does not reinterpret substituted data, and supports both plural branches", () => {
    expect(translate("en", "session.pin.failed", { detail: "<script>{count}</script>" })).toContain("<script>{count}</script>");
    expect(pluralKey("en", "session.questions.other", 1)).toBe("session.questions.one");
    expect(pluralKey("zh-CN", "session.questions.other", 1)).toBe("session.questions.other");
    expect(translate("en", "session.delete.title")).toBe(en["error.unknown"]);
  });
  it("defaults to existing English behavior until initialized", () => {
    expect(getLocale()).toBe("en");
    expect(normalizeLocale("zh_TW.UTF-8")).toBe("zh-CN");
    expect(normalizeLocale("fr-FR")).toBe("en");
  });
});
