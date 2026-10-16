import { describe, expect, it } from "vitest";

import { translate } from "../../../src/shared/i18n";
import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import { buildProviderCards } from "./providerCards";

function model(overrides: Partial<SettingsModelView> & Pick<SettingsModelView, "id">): SettingsModelView {
  return {
    api: "openai",
    apiKeyEnv: "DEEPSEEK_API_KEY",
    baseUrl: "https://api.deepseek.com",
    capabilities: { files: false, reasoning: true, tools: true, vision: false, webSearch: false },
    keyPresent: false,
    modelName: overrides.id,
    provider: "deepseek",
    source: "builtin",
    ...overrides,
  };
}

const deepseekPro = model({ id: "deepseek-v4-pro", keyPresent: true, keySource: "saved" });
const deepseekFlash = model({ id: "deepseek-v4-flash", keyPresent: true, keySource: "saved" });
const openaiBuiltin = model({ api: "openai-responses", apiKeyEnv: "OPENAI_API_KEY", baseUrl: "https://api.openai.com", id: "gpt-5.6", provider: "openai" });
const claudeBuiltin = model({ api: "anthropic-messages", apiKeyEnv: "ANTHROPIC_API_KEY", baseUrl: "https://api.anthropic.com", id: "claude-opus-5", provider: "anthropic" });

function relay(id: string, api: string, apiKeyEnv: string, extra: Partial<SettingsModelView> = {}): SettingsModelView {
  return model({ api, apiKeyEnv, baseUrl: "https://fcodex.example.com/v1", id, keyPresent: true, keySource: "saved", provider: "fcodex", source: "user", ...extra });
}

function cards(models: SettingsModelView[], roles: { defaultModelId?: string; utilityModelId?: string } = {}) {
  return buildProviderCards({ ...roles, models, presetLabel: (preset) => translate("en", preset.labelKey) });
}

describe("buildProviderCards", () => {
  it("shows one DeepSeek card for a DeepSeek-only user and folds every other preset into connectable", () => {
    const result = cards([deepseekPro, deepseekFlash, openaiBuiltin, claudeBuiltin]);
    expect(result.cards.map((card) => card.title)).toEqual(["DeepSeek"]);
    expect(result.cards[0].rows.map((row) => row.model.id)).toEqual(["deepseek-v4-pro", "deepseek-v4-flash"]);
    expect(result.cards[0].hosts).toEqual(["api.deepseek.com"]);
    expect(result.connectable.map((preset) => translate("en", preset.labelKey))).toEqual([
      "OpenAI", "Anthropic", "Kimi Code", "Zhipu AI", "Moonshot", "MiMo",
    ]);
    expect(JSON.stringify(result.cards)).not.toContain("claude");
  });

  it("keeps a single-key multi-protocol relay on one card with each row's own protocol", () => {
    const result = cards([
      relay("fcodex/gpt-6.1-sol", "openai-responses", "FCODEX_API_KEY"),
      relay("fcodex/claude-opus-5-5", "anthropic-messages", "FCODEX_API_KEY"),
    ]);
    expect(result.cards).toHaveLength(1);
    expect(result.cards[0]).toMatchObject({ hosts: ["fcodex.example.com"], keyEnv: "FCODEX_API_KEY", title: "Fcodex" });
    expect(result.cards[0].rows.map((row) => row.model.api)).toEqual(["openai-responses", "anthropic-messages"]);
  });

  it("numbers same-title cards by slot length, then name", () => {
    const twoKeys = cards([
      relay("fcodex/claude-fable-5", "anthropic-messages", "FCODEX_2_API_KEY"),
      relay("fcodex/gpt-6.1-sol", "openai-responses", "FCODEX_API_KEY"),
    ]);
    expect(twoKeys.cards.map((card) => [card.title, card.keyEnv])).toEqual([
      ["Fcodex", "FCODEX_API_KEY"],
      ["Fcodex 2", "FCODEX_2_API_KEY"],
    ]);
    expect(twoKeys.cards.every((card) => card.hosts.join() === "fcodex.example.com")).toBe(true);

    const legacy = cards([
      relay("fcodex/claude", "anthropic-messages", "FCODEX_ANTHROPIC_API_KEY"),
      relay("fcodex/gpt", "openai-responses", "FCODEX_OPENAI_API_KEY"),
    ]);
    expect(legacy.cards.map((card) => [card.title, card.keyEnv])).toEqual([
      ["Fcodex", "FCODEX_OPENAI_API_KEY"],
      ["Fcodex 2", "FCODEX_ANTHROPIC_API_KEY"],
    ]);

    const many = cards(["FCODEX_10_API_KEY", "FCODEX_9_API_KEY", "FCODEX_API_KEY"].map((slot) => relay(`fcodex/${slot}`, "openai", slot)));
    expect(many.cards.map((card) => card.keyEnv)).toEqual(["FCODEX_API_KEY", "FCODEX_9_API_KEY", "FCODEX_10_API_KEY"]);
    expect(many.cards.map((card) => card.title)).toEqual(["Fcodex", "Fcodex 2", "Fcodex 3"]);

    const differentHosts = cards([
      relay("a/x", "openai", "TEAM_API_KEY", { baseUrl: "https://a.example.com", provider: "team" }),
      relay("b/x", "openai", "TEAM_B_API_KEY", { baseUrl: "https://b.example.com", provider: "team" }),
    ]);
    expect(differentHosts.cards.map((card) => card.title)).toEqual(["Team", "Team 2"]);
  });

  it("separates missing-key cards with user models from unconnected built-in slots", () => {
    const legacyUser = model({ api: "openai-responses", apiKeyEnv: "OPENAI_API_KEY", baseUrl: "https://api.openai.com", id: "gpt-5.4", provider: "openai", source: "user" });
    const result = cards([deepseekPro, legacyUser, openaiBuiltin, claudeBuiltin]);
    expect(result.cards.map((card) => [card.title, card.keyPresent])).toEqual([["DeepSeek", true], ["OpenAI", false]]);
    expect(result.cards[1].rows.map((row) => row.model.id)).toEqual(["gpt-5.4", "gpt-5.6"]);
    expect(result.cards[1].keySource).toBeNull();
    expect(result.connectable.map((preset) => preset.provider)).not.toContain("openai");
    expect(result.connectable.map((preset) => preset.provider)).toContain("anthropic");
  });

  it("keeps all seven presets connectable when the catalog shrinks or is empty", () => {
    expect(cards([]).connectable).toHaveLength(7);
    expect(cards([]).cards).toEqual([]);
  });

  it("files a built-in moved to a relay key under the relay card and lists both hosts", () => {
    const moved = model({ ...openaiBuiltin, apiKeyEnv: "FCODEX_API_KEY", keyPresent: true, keySource: "saved" });
    // list_models keeps a built-in override at its catalog position, ahead of user entries.
    const result = cards([moved, relay("fcodex/claude-opus-5-5", "anthropic-messages", "FCODEX_API_KEY")]);
    expect(result.cards).toHaveLength(1);
    expect(result.cards[0].title).toBe("Fcodex");
    expect(result.cards[0].anchor.id).toBe("fcodex/claude-opus-5-5");
    expect(result.cards[0].rows.map((row) => row.model.id)).toEqual(["gpt-5.6", "fcodex/claude-opus-5-5"]);
    expect(result.cards[0].hosts).toEqual(["fcodex.example.com", "api.openai.com"]);
    expect(result.connectable.map((preset) => preset.provider)).toContain("openai");
  });

  it("marks default and utility rows and reports an environment key source", () => {
    const envClaude = model({ ...claudeBuiltin, keyPresent: true, keySource: "environment" });
    const result = cards([deepseekPro, deepseekFlash, envClaude], { defaultModelId: "deepseek-v4-pro", utilityModelId: "deepseek-v4-flash" });
    const deepseek = result.cards.find((card) => card.title === "DeepSeek")!;
    expect(deepseek.rows.map((row) => [row.model.id, row.isDefault, row.isUtility])).toEqual([
      ["deepseek-v4-pro", true, false],
      ["deepseek-v4-flash", false, true],
    ]);
    expect(deepseek.keySource).toBe("saved");
    expect(result.cards.find((card) => card.title === "Anthropic")?.keySource).toBe("environment");
  });
});
