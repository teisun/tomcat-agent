import { describe, expect, it } from "vitest";

import { translate } from "../../../src/shared/i18n";
import {
  findMatchingProviderPreset,
  findPresetByKeyEnv,
  humanizeProviderLabel,
  PROVIDER_PRESETS,
} from "./providerPresets";

describe("providerPresets", () => {
  it("is a static table of seven presets that never depends on the built-in catalog", () => {
    expect(PROVIDER_PRESETS.map((preset) => [translate("en", preset.labelKey), preset.baseUrl, preset.api, preset.apiKeyEnv])).toEqual([
      ["DeepSeek", "https://api.deepseek.com", "openai", "DEEPSEEK_API_KEY"],
      ["OpenAI", "https://api.openai.com/v1", "openai-responses", "OPENAI_API_KEY"],
      ["Anthropic", "https://api.anthropic.com", "anthropic-messages", "ANTHROPIC_API_KEY"],
      ["Kimi Code", "https://api.kimi.com/coding/v1", "openai", "KIMI_API_KEY"],
      ["Zhipu AI", "https://open.bigmodel.cn/api/paas/v4", "openai", "ZHIPU_API_KEY"],
      ["Moonshot", "https://api.moonshot.cn/v1", "openai", "MOONSHOT_API_KEY"],
      ["MiMo", "https://token-plan-cn.xiaomimimo.com", "openai", "MIMO_API_KEY"],
    ]);
    expect(translate("zh-CN", "models.preset.zhipu")).toBe("智谱");
    expect(findPresetByKeyEnv(" KIMI_API_KEY ")?.provider).toBe("kimi");
    expect(findPresetByKeyEnv("FCODEX_API_KEY")).toBeNull();
  });

  it("matches a model to a preset by provider plus protocol, tolerating a different literal URL", () => {
    expect(findMatchingProviderPreset({ api: "openai-responses", baseUrl: "https://api.openai.com", provider: "openai" })?.provider).toBe("openai");
    expect(findMatchingProviderPreset({ api: "anthropic-messages", baseUrl: "https://api.anthropic.com", provider: "anthropic" })?.provider).toBe("anthropic");
    expect(findMatchingProviderPreset({ api: "openai", baseUrl: "https://api.chatanywhere.tech/v1", provider: "openai" })).toBeNull();
  });

  it("humanizes non-preset provider names for card titles", () => {
    expect(humanizeProviderLabel("fcodex")).toBe("Fcodex");
    expect(humanizeProviderLabel("team-gateway")).toBe("Team Gateway");
    expect(humanizeProviderLabel(" ")).toBe("Custom");
  });
});
